//! The streaming turn loop for [`ConversationRuntime`] — the `RenderBlock`-emitting
//! counterpart to the synchronous `run_turn` path in `mod.rs`. Split out verbatim
//! so `mod.rs` reads as the sync orchestrator; behaviour-preserving. The three
//! empty-stream helpers here are `pub(super)` because the sync loop in `mod.rs`
//! shares them.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::sync::mpsc;

use super::{
    ask_user_question_async, budget_exhausted_notice, build_assistant_message,
    build_async_permission_request, collect_pending_tool_uses, empty_stream_exhausted_message,
    batch_had_successful_mutation, format_tool_result_from_raw, permission_denial_notice,
    is_refusal_stop_reason, is_truncation_stop_reason, is_verify_class_tool, merge_hook_context,
    merge_hook_feedback,
    normalize_empty_assistant_stream,
    parallel_waves, pre_hook_denial_outcome, ConcurrentDispatchFn,
    overload_demotion_warn, quota_fallback_swap_warn, quota_wait_hold_warn,
    refusal_surfaced_message,
    sleep_tool_execution_input,
    agent_notification_text, steering_message, tool_execution_input,
    take_truncation_continuation, tool_preview_from,
    tool_result_message, tool_summary_line, unblock_tool_execute, ApiClient, AssistantEvent,
    AssistantTurn, AsyncPermissionDecision, AsyncPermissionPrompter, BlockIdGen, BudgetExhausted,
    CapturePrompter, ContentBlock, ConversationMessage, ConversationRuntime, EmptyAssistantAction,
    HookEvent, HookRunResult, PermissionContext, PermissionOutcome, PermissionPromptDecision,
    PromptCacheEvent, QuotaEscape, RefusalDecision, RenderBlock, RuntimeError, SteeringQueue,
    StreamingTurnError, SystemLevel,
    TokenUsage, ToolBatchRepetitionHardStops, ToolCallId, ToolCallStatus, ToolExecutor,
    TurnContinuity, TurnSummary,
    DEFAULT_STREAMING_CHANNEL_CAPACITY,
    EMPTY_STREAM_CONTINUATION_REMINDER, EMPTY_STREAM_CONTINUATION_REMINDER_PREFIX,
    EMPTY_STREAM_EXHAUSTED_FALLBACK_TEXT, EMPTY_STREAM_RETRY_REMINDER,
    EMPTY_STREAM_RETRY_REMINDER_PREFIX, EMPTY_STREAM_TRUNCATION_RETRY_REMINDER,
    MAX_EMPTY_STREAM_RETRIES, MAX_PARALLEL_SAFE_TOOL_DISPATCHES, REFUSAL_DRY_PREARM_WARN,
    REFUSAL_FALLBACK_WARN, REFUSAL_SURFACED_NOTICE, STEERING_ECHO_PREFIX,
    TRUNCATION_CONTINUATION_REMINDER,
};
use crate::message_stream::types::WireModel;
use crate::conversation::error::ToolTextKind;
use crate::message_stream::ToolResultBody;
use crate::tool_cancel::{ToolCancelWatch, ToolDispatchOutcome};

/// Await one `spawn_blocking` tool dispatch, racing it against a user cancel
/// (Esc once — "drop this tool, keep the turn").
///
/// ## What cancellation can and cannot do here
///
/// A `spawn_blocking` task **cannot be aborted**: `JoinHandle::abort` is a
/// no-op once the closure is on a blocking thread, and dropping the handle only
/// detaches it. So this function does two honest things and does not pretend to
/// do a third:
///
/// 1. It signals every foreground bash process group owned by this session, so
///    the overwhelmingly common case — a wedged shell command — really dies.
/// 2. It stops *waiting*. The turn moves on from the synthetic result at once,
///    which is the behavior the user asked for.
///
/// What it does **not** do is unwind a tool with no kill route — a hung MCP
/// call, an in-flight HTTP request, a long local computation. Those keep running
/// on their blocking thread until they finish on their own, and their eventual
/// return value is dropped on the floor. That is a real cost and it is chosen
/// deliberately: the alternative is the status quo, where the user's only exit
/// from a wedged MCP tool is killing the whole turn. Side effects already
/// committed by such a tool (a file written, a request sent) stand — the
/// synthetic result says "cancelled", not "did nothing".
async fn await_cancellable_tool_dispatch(
    handle: tokio::task::JoinHandle<(String, ToolTextKind)>,
    watch: &mut ToolCancelWatch,
    owner_session: &str,
) -> ToolDispatchOutcome {
    tokio::select! {
        // Biased so a dispatch that already completed settles as itself even if
        // a cancel lands in the same poll: a finished tool has a real answer and
        // reporting it as cancelled would throw away work already paid for.
        biased;
        joined = handle => match joined {
            Ok(result) => ToolDispatchOutcome::Completed(result.0, result.1),
            Err(join_error) => ToolDispatchOutcome::Completed(join_error.to_string(), ToolTextKind::Failure),
        },
        () = watch.cancelled() => {
            crate::bash::interrupt_foreground_bash(Some(owner_session));
            ToolDispatchOutcome::Cancelled
        }
    }
}

/// The body a tool's error text is drawn as. A declined verdict is its own
/// body, so the screen can head it as a verdict rather than a failure; every
/// other error is the tool's formatted error result.
fn tool_error_body(tool_name: &str, output: String, kind: ToolTextKind) -> ToolResultBody {
    if kind == ToolTextKind::Declined {
        ToolResultBody::Declined { reason: output }
    } else {
        format_tool_result_from_raw(tool_name, &output, true)
    }
}

// Rough token estimate for the system prompt sections.
//
// Uses the same heuristic as `estimate_message_tokens` (chars / 4) so
// that `build_request`'s overflow guard accounts for the system prompt
// ============================================================================
// Streaming variant — L7a
// ============================================================================
//
// Section header cites `.zo/code-rules.md` R-numbers that this block
// satisfies:
//
// * R1 (RenderBlock is the only currency that crosses the tui boundary).
// * R3 (no `block_on` / `block_in_place` inside an async context — we
//   reach the async prompter exclusively via `.await`).
// * R6 (stream pipeline errors propagate, never panic, never silently
//   drop).
// * R8 (bounded mpsc channels only — see
//   [`DEFAULT_STREAMING_CHANNEL_CAPACITY`]).

/// Wall-clock ceiling on ONE interactive permission prompt — the turn's only
/// unbounded await on a human (`prompter.decide(..).await` below).
///
/// This is where a measured 20-minute stall actually lived: an escalation
/// prompt (`workspace-write` → `danger-full-access`, `permissions.rs:428`) was
/// rendered for an MCP tool, nothing answered it, and the turn sat on the
/// prompt until the session was killed. The tool was never dispatched, so no
/// timeout further down the tool path could have helped.
///
/// The DEFAULT is disabled (wait forever): a permission prompt is a question
/// to a human, and auto-denying a user who stepped away for a few minutes
/// would lose their action (Claude Code waits indefinitely here too). Set
/// `ZO_PERMISSION_PROMPT_TIMEOUT_SECS=N` to bound the wait for automation —
/// benches, PTY drivers, and headless hosts where nobody can answer. On
/// expiry the prompt resolves as a denial — an ordinary tool error result, so
/// the turn continues — and the expiry text is deliberately retry-friendly
/// (unlike a mode-based denial) because the same call can succeed once a
/// human is back to approve it.
const PERMISSION_PROMPT_TIMEOUT_SECS: u64 = 0;

/// Env override for [`PERMISSION_PROMPT_TIMEOUT_SECS`].
const PERMISSION_PROMPT_TIMEOUT_ENV: &str = "ZO_PERMISSION_PROMPT_TIMEOUT_SECS";

/// Resolve the prompt ceiling; `None` = disabled (default, and `…=0`), i.e.
/// wait forever. Unparseable values fall back to the disabled default — a
/// typo must never silently start auto-denying an interactive user's prompts.
pub(super) fn permission_prompt_timeout() -> Option<std::time::Duration> {
    let seconds = std::env::var(PERMISSION_PROMPT_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(PERMISSION_PROMPT_TIMEOUT_SECS);
    (seconds > 0).then(|| std::time::Duration::from_secs(seconds))
}

/// Model-facing reason for a prompt that expired unanswered. Phrased as a
/// *transient* outcome on purpose: a mode-based denial is deterministic and
/// tells the model never to retry, but an unanswered prompt only means nobody
/// was at the keyboard, and the same call can succeed once approved.
fn permission_prompt_expired_reason(tool_name: &str, budget: std::time::Duration) -> String {
    format!(
        "permission prompt for '{tool_name}' expired after {}s with no answer — nobody approved \
         or denied it, so the call was not run. This is a timeout, not a decision: ask the user to \
         approve (TUI: `/permissions`) before trying again, or take an approach that needs no \
         escalation.",
        budget.as_secs()
    )
}

// ============================================================================
// Mid-generation steering re-issue (gen-abort)
// ============================================================================

/// How often the mid-generation steering check re-runs while the provider
/// stream is producing nothing at all.
///
/// The check is event-driven first: it re-runs after every block the provider
/// streams, which is the common case (text/reasoning deltas arrive
/// continuously). This tick only covers the silent window — a model that
/// thinks for many seconds without emitting anything is exactly when a user
/// gives up waiting and types a correction, and that steer should not sit
/// behind the silence. It is armed only while a stream is in flight, so it
/// costs nothing between calls.
const STEERING_INTERRUPT_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(120);

/// Whether the user has typed steering that no boundary has folded yet.
///
/// The read-only counterpart to `drain_steering`: the mid-generation check
/// runs on every streamed block, so it must never *take* the queue (a drain
/// there would strand the steer whenever the re-issue is declined) and must
/// never panic on a poisoned lock — an unreadable queue reads as "no
/// steering", exactly like the drain does.
fn steering_pending(queue: &SteeringQueue) -> bool {
    queue.lock().map(|queue| !queue.is_empty()).unwrap_or(false)
}

/// How a streaming provider call stopped being consumed.
enum StreamRace {
    /// The call ran to its own end — success or provider error. Everything
    /// downstream (retry ladder, quota escape, refusal ladder, empty-stream
    /// recovery, bookkeeping) sees exactly what it saw before this race
    /// existed.
    Completed(Result<Vec<AssistantEvent>, RuntimeError>),
    /// The user steered while the model was still generating, before this call
    /// had emitted a single `tool_use` block, so the in-flight call was
    /// abandoned to be re-issued with the steering folded in.
    ///
    /// Deliberately its own variant rather than an error, because it is none
    /// of the three things the loop already knows how to unwind: not a
    /// provider failure (so no retry counter, no fallback ladder, no quota
    /// escape), not a user cancel ([`ConversationRuntime::cancel_streaming_turn_by_user`]
    /// ends the turn), and not a render-channel drop
    /// ([`ConversationRuntime::cancel_streaming_turn`] truncates the
    /// transcript). Nothing from the abandoned call is settled; the turn
    /// continues with a fresh request built from the same conversation.
    SteeringReissue {
        /// The text block this call left mid-stream, if any, so the caller can
        /// close it before the re-issued call opens a new one. Carries the id
        /// the provider actually used rather than assuming the iteration's
        /// reserved one: a turn that opens with reasoning spends that id on the
        /// `Reasoning` block and streams its text under a later one.
        open_text_block: Option<crate::message_stream::types::BlockId>,
    },
}

/// Drive one provider stream to completion while forwarding its render blocks,
/// racing it against the mid-turn steering queue.
///
/// `stream` writes its blocks into `probe_rx`'s channel rather than straight
/// to `render_tx`; this function forwards them one by one. The interposition
/// buys exactly two facts that cannot be observed from outside the call:
///
/// * the instant the first `tool_use` block starts streaming (`ToolCall`), and
/// * a place on the *same* task to re-check the steering queue as the call
///   progresses.
///
/// Forwarding is awaited, so end-to-end backpressure (code-rules R8) is
/// unchanged. A consumer that drops the render receiver mid-stream closes the
/// probe channel, which makes the provider's next send fail exactly as a
/// direct send would have — the existing stream-error path stays in charge of
/// that case.
///
/// Returns as soon as the stream resolves *or* the re-issue conditions hold.
/// The caller owns the future, so dropping it (which tears down the upstream
/// connection) and draining any blocks still buffered are the caller's job.
async fn race_stream_against_steering<F>(
    stream: &mut F,
    probe_rx: &mut mpsc::Receiver<RenderBlock>,
    render_tx: &mpsc::Sender<RenderBlock>,
    steering: &SteeringQueue,
    abort: &std::sync::atomic::AtomicBool,
    interrupt_armed: bool,
    stamps: &mut crate::request_timings::StreamStamps,
) -> StreamRace
where
    F: std::future::Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Unpin,
{
    // A call that has begun emitting tool calls is past the point where
    // re-issuing pays: the batch is nearly assembled, and the existing
    // tool-result boundary fold delivers the steer before those tools' results
    // are ever reasoned about. Only a call still purely generating is worth
    // abandoning.
    let mut saw_tool_use = false;
    let mut open_text_block = None;
    let mut probe_open = true;
    let mut forwarding = true;
    loop {
        tokio::select! {
            // The provider always wins the race when it is ready: a call that
            // has already finished must never be reported as interrupted.
            biased;
            outcome = &mut *stream => {
                stamps.complete();
                return StreamRace::Completed(outcome);
            }
            block = probe_rx.recv(), if probe_open => {
                match block {
                    Some(block) => {
                        // Everything the provider has already sent counts
                        // before the decision below. The stream-phase block
                        // that precedes every request (r46) sits in front of
                        // the first tool call; judging after that block alone
                        // abandoned a call whose tool_use was already on the
                        // channel (steering_gen_abort, 2026-09-04).
                        let mut ready = Some(block);
                        while let Some(block) = ready.take() {
                            // The timing ledger reads the probe channel too:
                            // the phase block opens the attempt, the first
                            // provider block is the first byte, the first
                            // text/reasoning/tool block the first visible.
                            stamps.observe(&block);
                            match &block {
                                RenderBlock::ToolCall { .. } => saw_tool_use = true,
                                RenderBlock::TextDelta { id, done, .. } => {
                                    open_text_block = (!done).then_some(*id);
                                }
                                _ => {}
                            }
                            if forwarding && render_tx.send(block).await.is_err() {
                                // The render consumer is gone. Close the probe
                                // so the provider's next send fails the way a
                                // direct send would have, then keep draining so
                                // it is never blocked on a full channel while
                                // it unwinds.
                                forwarding = false;
                                probe_rx.close();
                            }
                            ready = probe_rx.try_recv().ok();
                        }
                    }
                    None => probe_open = false,
                }
            }
            () = tokio::time::sleep(STEERING_INTERRUPT_POLL_INTERVAL) => {}
        }
        // A turn already being torn down (Ctrl+C, a hook abort, a host
        // failure) must not buy one more provider call on the way out: the
        // steering it would carry has nowhere to land.
        if interrupt_armed
            && !saw_tool_use
            && !abort.load(std::sync::atomic::Ordering::SeqCst)
            && steering_pending(steering)
        {
            return StreamRace::SteeringReissue { open_text_block };
        }
    }
}

/// How long a turn waits for the network to come back before giving up —
/// the length of a coffee break, a tunnel, a router reboot. Not a retry
/// budget: nothing is re-sent while waiting, only a TCP probe.
const NETWORK_WAIT_CEILING: Duration = Duration::from_secs(30 * 60);
const NETWORK_WAIT_INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const NETWORK_WAIT_MAX_BACKOFF: Duration = Duration::from_secs(15);
const NETWORK_PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const NETWORK_RESTORED_INFO: &str = "connection restored — continuing (the interrupted answer starts over)";

fn network_wait_warn(host: &str, delay: Duration, waited: Duration, attempt: u32) -> String {
    let target = if host.is_empty() { "the provider" } else { host };
    format!(
        "network unreachable ({target}); waiting for the connection — next check in {}s, \
         waited {}s (attempt {attempt})",
        delay.as_secs(),
        waited.as_secs()
    )
}

/// Does the network carry a connection toward `host` right now? A completed
/// or refused TCP connect says yes; DNS failure, no route, or a probe that
/// times out says no (this machine's stealth firewall drops a closed port's
/// SYN rather than refusing it, so a timeout is the common "no" even for a
/// live link to a dead port — the provider's port is never dead). An empty
/// host — an error that named none — is answered optimistically after one
/// backoff, so the re-run itself becomes the probe.
async fn network_reaches(host: &str) -> bool {
    network_reaches_port(host, 443).await
}

async fn network_reaches_port(host: &str, port: u16) -> bool {
    if host.is_empty() {
        return true;
    }
    let connect = tokio::net::TcpStream::connect((host, port));
    match tokio::time::timeout(NETWORK_PROBE_TIMEOUT, connect).await {
        Ok(Ok(_)) => true,
        Ok(Err(error)) => error.kind() == std::io::ErrorKind::ConnectionRefused,
        Err(_) => false,
    }
}

struct PreparedStreamingTool {
    tool_use_id: String,
    tool_name: String,
    effective_input: String,
    pre_hook_result: HookRunResult,
    permission_outcome: PermissionOutcome,
    tool_call_id: ToolCallId,
}

struct PrecomputedStreamingToolResult {
    output: String,
    kind: ToolTextKind,
    tool_start: std::time::Instant,
}

struct StreamingToolRenderContext<'a> {
    render_tx: &'a mpsc::Sender<RenderBlock>,
    id_gen: &'a BlockIdGen,
    rollback_message_count: usize,
}

struct StreamingToolFinalizeOptions {
    tool_start: std::time::Instant,
    render_result: bool,
    notify_slow: bool,
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Streaming variant of [`ConversationRuntime::run_turn`].
    ///
    /// Emits [`RenderBlock`] values into `render_tx` as the agent loop
    /// progresses (text deltas, tool-call cards, tool results, system
    /// notices) and returns a [`TurnSummary`] on completion — matching
    /// the logical sequence the synchronous `run_turn` path produces
    /// for identical inputs.
    ///
    /// # Permission model
    ///
    /// When the permission policy needs human approval, the loop
    /// awaits `prompter.decide(..)`. Any
    /// [`crate::permission::PermissionPrompter`] implementation works;
    /// the canonical choice is L3's `ChannelPrompter`, which forwards
    /// requests over a bounded mpsc to the TUI event loop. No
    /// modification to L3 was required.
    ///
    /// # Cancellation
    ///
    /// If the consumer drops the `RenderBlock` receiver at any point,
    /// the next send fails with a closed-channel error and the loop
    /// returns [`StreamingTurnError::Cancelled`]. No new tool dispatch
    /// is started after cancellation is observed; an already-running
    /// tool will finish its in-flight execution before the loop
    /// notices the drop, but its result is simply discarded.
    ///
    /// # Backpressure
    ///
    /// `render_tx` is bounded (see [`DEFAULT_STREAMING_CHANNEL_CAPACITY`]).
    /// If the TUI cannot keep up, `send().await` suspends the loop
    /// until the consumer drains the channel, giving honest end-to-end
    /// backpressure per code-rules R8.
    ///
    /// # Relationship to sync `run_turn`
    ///
    /// This remains an additive, parallel implementation: the synchronous
    /// [`ConversationRuntime::run_turn`] and this streaming loop stay as
    /// separate orchestration paths so the non-TTY `-p` CLI path and the
    /// `RenderBlock` emission boundary do not get semantically merged. Small
    /// phase helpers are shared only where the two paths are byte-identical or
    /// trivially parameterized (empty-response cleanup/retry state, truncation
    /// continuation gating, pre-hook denial mapping, concurrency-safe batch
    /// selection, and final tool-result message construction). Streaming-only
    /// rendering, async permission prompts, request assembly, cancellation, and
    /// steering remain local to this loop. See
    /// `.zo/tasks/L7a-runtime-streaming.handoff.md` for the original
    /// rationale behind the "duplicate the loop body" decision.
    // cohesive async streaming turn; loop body intentionally parallel to the
    // sync path with only identical/parameterized phase helpers shared
    pub async fn run_turn_streaming(
        &mut self,
        user_input: impl Into<String>,
        render_tx: mpsc::Sender<RenderBlock>,
        prompter: Arc<dyn AsyncPermissionPrompter>,
    ) -> Result<TurnSummary, StreamingTurnError> {
        self.run_turn_streaming_with_images(user_input, Vec::new(), render_tx, prompter)
            .await
    }

    /// Like [`run_turn_streaming`] but with optional image attachments.
    ///
    /// Each image is a `(media_type, base64_data)` pair prepended to the
    /// user message before the text block.
    // cohesive async streaming turn (image-carrying variant); see the sibling
    // run_turn_streaming note on the intentional loop-body duplication
    #[allow(clippy::too_many_lines)]
    /// Clear the transient "empty stream retry" system reminder if one was set
    /// this turn. No-op when `empty_retries == 0`; shared by the many early-exit
    /// paths of both turn loops so the cleanup lives in one place.
    pub(super) fn clear_empty_retry_reminder(&mut self, empty_retries: usize) {
        if empty_retries > 0 {
            self.replace_transient_system_reminder_by_prefix(
                EMPTY_STREAM_RETRY_REMINDER_PREFIX,
                None,
            );
        }
    }

    pub(super) fn accept_assistant_content_turn(
        &mut self,
        empty_retries: &mut usize,
        empty_recovery_attempted: &mut bool,
        message: ConversationMessage,
        usage: Option<TokenUsage>,
        prompt_cache_events: Vec<PromptCacheEvent>,
        stop_reason: Option<String>,
    ) -> (
        ConversationMessage,
        Option<TokenUsage>,
        Vec<PromptCacheEvent>,
        Option<String>,
    ) {
        self.clear_empty_retry_reminder(*empty_retries);
        self.replace_transient_system_reminder_by_prefix(
            EMPTY_STREAM_CONTINUATION_REMINDER_PREFIX,
            None,
        );
        *empty_retries = 0;
        *empty_recovery_attempted = false;
        (message, usage, prompt_cache_events, stop_reason)
    }

    pub(super) fn handle_empty_assistant_turn(
        &mut self,
        usage: Option<TokenUsage>,
        stop_reason: Option<&str>,
        empty_retries: &mut usize,
        empty_recovery_attempted: &mut bool,
    ) -> EmptyAssistantAction {
        if let Some(usage) = usage {
            self.usage_tracker.record(usage);
        }
        if *empty_retries < MAX_EMPTY_STREAM_RETRIES {
            *empty_retries += 1;
            let reminder = if stop_reason.is_some_and(is_truncation_stop_reason) {
                EMPTY_STREAM_TRUNCATION_RETRY_REMINDER
            } else {
                EMPTY_STREAM_RETRY_REMINDER
            };
            self.replace_transient_system_reminder_by_prefix(
                EMPTY_STREAM_RETRY_REMINDER_PREFIX,
                Some(reminder),
            );
            return EmptyAssistantAction::Retry;
        }
        self.replace_transient_system_reminder_by_prefix(
            EMPTY_STREAM_CONTINUATION_REMINDER_PREFIX,
            Some(EMPTY_STREAM_CONTINUATION_REMINDER),
        );
        self.replace_transient_system_reminder_by_prefix(EMPTY_STREAM_RETRY_REMINDER_PREFIX, None);
        if *empty_recovery_attempted {
            EmptyAssistantAction::Exhausted
        } else {
            *empty_recovery_attempted = true;
            *empty_retries = 0;
            EmptyAssistantAction::ContinueOnce
        }
    }

    /// Turn-start prologue for [`Self::run_turn_streaming_with_images`]: record
    /// turn-started telemetry, capture the output-token baseline and the pre-turn
    /// message index (for rollback on an early failure), push the user input
    /// (text-only or image-carrying), and reset the per-turn transient state.
    ///
    /// Returns `(turn_start_output_tokens, message_count_before)`. The streaming
    /// loop differs from the sync [`Self::begin_turn_once`] in that it must also
    /// expose `message_count_before` so a first-iteration stream failure can
    /// truncate the orphaned user message back off the session.
    pub(super) fn begin_streaming_turn(
        &mut self,
        user_input: String,
        images: Vec<(String, String)>,
        internal_subturn: bool,
    ) -> Result<(u32, usize), StreamingTurnError> {
        if !internal_subturn {
            self.tool_executor.begin_user_turn();
            self.run_user_prompt_submit_for_streaming_user_entry(&user_input)?;
        }
        // Before the user message is pushed — see `begin_attempt`.
        self.begin_attempt();
        self.record_turn_started(&user_input);
        let turn_start_output_tokens = self.usage_tracker.cumulative_usage().output_tokens;
        let message_count_before = self.session.messages.len();
        if images.is_empty() {
            self.session
                .push_user_text(user_input)
                .map_err(|error| StreamingTurnError::runtime(error.to_string()))?;
        } else {
            self.session
                .push_user_with_images(user_input, images)
                .map_err(|error| StreamingTurnError::runtime(error.to_string()))?;
        }

        self.tool_fingerprint_counts.clear();

        self.pathless_fingerprints.clear();

        self.serial_read_batches = 0;

        self.serial_reads_nudged = false;
        self.tool_repetition_pending_hard_stop_fps.clear();
        self.tool_repetition_hard_stop_fps.clear();
        self.read_file_ranges_by_path.clear();
        self.read_file_redundant_advised_paths.clear();
        self.cross_turn_tool_repetition_pending_hard_stop_fps.clear();
        self.mode_denial_counts.clear();
        self.tool_loop_break_requested = false;
        self.verify_treadmill_run = 0;
        self.turn_progress_results = 0;
        self.heuristic_stop_nudges = 0;
        self.progress_marker_at_last_signal = 0;
        self.full_compactions_this_turn = 0;
        self.begin_step_effort_turn(internal_subturn);
        // Fold refusal history only at a PUBLIC boundary. Internal subturns are
        // additional legs of the same user turn and must not double-count it.
        if !internal_subturn {
            self.fold_finished_refusal_turn();
        }
        // Reset the per-leg refusal override before deciding whether the
        // session cooldown should re-arm it below.
        self.refusal_fallback_model = None;
        // The same-model retry is a per-turn budget, exactly like the sync
        // entry point: without this reset a streaming session would spend its
        // one retry on the first refusal ever and surface every later one.
        self.refusal_same_model_retry_used = false;
        // Reset the per-turn quota fallback, pre-arming onto it when the session
        // is still inside a recorded quota-dry cooldown (applies to internal
        // subturns too — a quota-dry session applies to every leg). See
        // [`Self::begin_turn_quota_fallback`].
        self.begin_turn_quota_fallback();
        // Escalation freshness: keep a freshly-installed confidence-cascade
        // escalation for THIS turn, drop a stale one from a prior turn.
        // Internal deep-lane subturns run inside the escalated turn and are
        // exempt. See [`Self::begin_turn_escalation`].
        self.begin_turn_escalation(internal_subturn);
        // Refusal-dry sees the would-be wire model only after the ordinary
        // refusal reset and quota/escalation state have settled. It applies to
        // every leg, even though only public begins fold the streak above.
        self.begin_turn_refusal_fallback();
        // A fresh PUBLIC user turn (not an internal deep-lane subturn) is genuine
        // new intent: reset the cross-turn re-read tally so legitimately
        // re-reading the same file across separate user-driven turns never
        // accumulates into a false cross-turn stop. Internal subturns
        // (auto-continuation) deliberately KEEP the tally so their own re-read
        // loop — the one this guard exists to catch — still trips it.
        if !internal_subturn {
            self.cross_turn_tool_fingerprints.clear();
            self.cross_turn_tool_repetition_pending_hard_stop_fps.clear();
            self.cross_turn_tool_repetition_hard_stop_fps.clear();
        }
        // NOTE: `consecutive_microcompacts` is intentionally NOT reset here — it
        // is a cross-turn thrash signal that must survive turn boundaries — see
        // `begin_turn_once`.
        Ok((turn_start_output_tokens, message_count_before))
    }

    pub(super) fn cancel_streaming_turn(
        &mut self,
        iteration: usize,
        reason: &str,
        message_count_before: usize,
    ) -> StreamingTurnError {
        let error = self.cancel_turn(iteration, reason);
        Arc::make_mut(&mut self.session.messages).truncate(message_count_before);
        self.session.mark_transcript_dirty();
        error
    }

    /// Settle a user/host abort, **keeping** everything the cancelled turn had
    /// already settled into the session.
    ///
    /// This deliberately does NOT roll back to the pre-turn message index.
    /// Rolling back destroyed the transcript of a cancelled turn: the user
    /// message and every settled assistant/tool message had already been
    /// appended to the bound JSONL by [`Session::push_message`], so the
    /// truncation-plus-`mark_transcript_dirty` combination made the host's
    /// next `persist_appended_state_to_path` re-publish the emptied in-memory
    /// state as a full snapshot — a ten-minute turn collapsed to a header-only
    /// stub and `/resume` came back with nothing.
    ///
    /// Retaining the prefix is wire-safe. A `tool_use` left without its result
    /// by the abort is sealed per request by
    /// [`Session::tool_consistent_messages`], which injects a synthetic
    /// interrupted result into the outgoing view without mutating stored
    /// history — so the next request is well-formed even though the transcript
    /// keeps the interrupted work.
    ///
    /// The unwinding *error* paths that are not user/host cancellation
    /// ([`Self::cancel_streaming_turn`]) still roll back, unchanged.
    fn settle_streaming_abort(&mut self, iteration: usize, reason: &str) -> StreamingTurnError {
        if !self.hook_abort_signal.is_handled() {
            match self.hook_abort_signal.origin() {
                Some(crate::HookAbortOrigin::User) => {
                    self.record_turn_cancelled(iteration, reason);
                }
                Some(crate::HookAbortOrigin::Host) | None => {
                    self.record_turn_host_failure(iteration, reason);
                }
            }
            self.hook_abort_signal.mark_handled();
        }
        StreamingTurnError::Cancelled
    }

    /// A connected gateway's content filter refused this request. When part of
    /// it was zo's own — the memory it recalled into the request — take that
    /// reminder back, say so once, and answer `true` so the caller asks again
    /// without it; the rest of the session asks that gateway without recall.
    /// When none was, the gateway refused the conversation itself: say that,
    /// and the way past it, beside the raw error the turn ends with (`false`).
    /// First-party models never take this road
    /// ([`Self::withhold_recall_from_refusing_gateway`]).
    async fn answer_gateway_refusal(
        &mut self,
        error: &RuntimeError,
        recall_attached: bool,
        keep: usize,
        render_tx: &mpsc::Sender<RenderBlock>,
        id_gen: &BlockIdGen,
    ) -> bool {
        let (text, ask_again) = if let Some(gateway) =
            self.withhold_recall_from_refusing_gateway(error, recall_attached, keep)
        {
            (
                format!(
                    "{gateway}'s content filter refused this request — the memory zo recalled into it is withheld from {gateway} for this session, and the request goes again without it"
                ),
                true,
            )
        } else if let Some(gateway) = self.gateway_refusing_the_conversation(error) {
            (
                format!(
                    "{gateway}'s content filter refused this request with no recalled memory in it — what it refused is the conversation itself. {gateway} decides what it accepts: rephrase, or /model to another provider"
                ),
                false,
            )
        } else {
            return false;
        };
        let _ = render_tx
            .send(RenderBlock::System {
                id: id_gen.next(),
                level: SystemLevel::Warn,
                text,
            })
            .await;
        ask_again
    }

    /// Wait for the network to carry a connection to `host` again, telling the
    /// transcript what is being waited for, and answer whether it came back
    /// before [`NETWORK_WAIT_CEILING`] or the turn was aborted.
    ///
    /// The probe is a TCP connect to the provider's port: a link that is up
    /// completes it, or is refused, within the probe timeout; a link that is
    /// down fails DNS or times out. A refusal counts as "back" — the network
    /// carried the packet — and the re-run then sees whatever the provider
    /// has to say. Backoff starts short and stays short: a link that returns
    /// should be used within seconds, not minutes.
    async fn await_network_return(
        &mut self,
        render_tx: &mpsc::Sender<RenderBlock>,
        id_gen: &BlockIdGen,
        host: &str,
    ) -> bool {
        let started = std::time::Instant::now();
        let mut delay = NETWORK_WAIT_INITIAL_BACKOFF;
        let mut attempt: u32 = 0;
        loop {
            if self.hook_abort_signal.is_aborted() {
                return false;
            }
            attempt += 1;
            let _ = render_tx
                .send(RenderBlock::System {
                    id: id_gen.next(),
                    level: SystemLevel::Warn,
                    text: network_wait_warn(host, delay, started.elapsed(), attempt),
                })
                .await;
            tokio::time::sleep(delay).await;
            if network_reaches(host).await {
                let _ = render_tx
                    .send(RenderBlock::System {
                        id: id_gen.next(),
                        level: SystemLevel::Info,
                        text: NETWORK_RESTORED_INFO.to_string(),
                    })
                    .await;
                return true;
            }
            if started.elapsed() >= NETWORK_WAIT_CEILING {
                return false;
            }
            delay = (delay * 2).min(NETWORK_WAIT_MAX_BACKOFF);
        }
    }

    fn cancel_streaming_turn_if_aborted(
        &mut self,
        iteration: usize,
        reason: &str,
    ) -> Option<StreamingTurnError> {
        self.hook_abort_signal
            .is_aborted()
            .then(|| self.settle_streaming_abort(iteration, reason))
    }

    /// Cancel a streaming turn at an explicit user/host boundary while the
    /// caller owns the runtime future. This is the public counterpart to the
    /// render-channel failure path above: it preserves typed cancellation
    /// provenance and keeps the turn's settled transcript (see
    /// `Self::settle_streaming_abort`).
    pub fn cancel_streaming_turn_by_user(&mut self, reason: &str) -> StreamingTurnError {
        self.hook_abort_signal.abort();
        self.settle_streaming_abort(0, reason)
    }

    /// Abort a streaming turn because its host failed independently of the
    /// user. Keeping this distinct prevents transport/render failures from being
    /// filtered as non-actionable user cancellations by Dreamer.
    pub fn cancel_streaming_turn_by_host(&mut self, reason: &str) -> StreamingTurnError {
        self.hook_abort_signal.abort_host();
        self.settle_streaming_abort(0, reason)
    }

    #[allow(clippy::too_many_lines)] // cohesive streaming-turn core: ingest → stream → tool-loop → settle, one scope
    pub async fn run_turn_streaming_with_images(
        &mut self,
        user_input: impl Into<String>,
        images: Vec<(String, String)>,
        render_tx: mpsc::Sender<RenderBlock>,
        prompter: Arc<dyn AsyncPermissionPrompter>,
    ) -> Result<TurnSummary, StreamingTurnError> {
        let result = self
            .run_turn_streaming_with_images_inner(user_input, images, render_tx, prompter, false)
            .await;
        self.settle_team_inbox_turn_for_result(&result);
        result
    }

    pub(crate) async fn run_internal_subturn_streaming_with_images(
        &mut self,
        user_input: impl Into<String>,
        images: Vec<(String, String)>,
        render_tx: mpsc::Sender<RenderBlock>,
        prompter: Arc<dyn AsyncPermissionPrompter>,
    ) -> Result<TurnSummary, StreamingTurnError> {
        self.run_turn_streaming_with_images_inner(user_input, images, render_tx, prompter, true)
            .await
    }

    #[allow(clippy::too_many_lines)] // cohesive streaming-turn core: ingest → stream → tool-loop → settle, one scope
    async fn run_turn_streaming_with_images_inner(
        &mut self,
        user_input: impl Into<String>,
        images: Vec<(String, String)>,
        render_tx: mpsc::Sender<RenderBlock>,
        prompter: Arc<dyn AsyncPermissionPrompter>,
        internal_subturn: bool,
    ) -> Result<TurnSummary, StreamingTurnError> {
        let id_gen = BlockIdGen::default();
        let user_input = user_input.into();
        // Baseline token count + pre-turn message index for rollback (see
        // [`Self::begin_streaming_turn`]). Internal deep-lane subturns carry
        // program-generated prompts, so they preserve the pre-A1 streaming
        // lifecycle behavior: no TurnStart or UserPromptSubmit policy.
        let (turn_start_output_tokens, message_count_before) =
            self.begin_streaming_turn(user_input, images, internal_subturn)?;
        // Input-side baseline for the third cost breaker; captured here (after
        // `begin_streaming_turn`, which makes no provider call) so both token
        // baselines describe the same instant.
        let turn_start_input_tokens = self.usage_tracker.cumulative_usage().input_tokens;

        let mut assistant_messages = Vec::new();
        let mut tool_results = Vec::new();
        let mut prompt_cache_events = Vec::new();
        let mut iterations = 0;
        let mut tool_calls = 0;
        let mut empty_retries = 0;
        let mut empty_recovery_attempted = false;
        let mut truncation_continuations = 0;
        let mut turn_end_gate_reprompts = 0;
        let mut auto_compaction = None;
        let mut provider_overflow_recovery_attempted = false;
        let mut microcompact = None;
        let mut budget_exhausted: Option<BudgetExhausted> = None;
        // Progress-gated deadline extensions granted so far this turn, and the
        // progress-result count at the last grant — each further extension
        // requires FRESH progress since the previous window, so a turn that
        // stops externalizing progress stops earning extensions.
        let mut deadline_extensions_used: u8 = 0;
        let mut extension_progress_marker: usize = 0;
        // Gen-abort cap. Set when a call is abandoned mid-generation to deliver
        // steering early (see [`StreamRace::SteeringReissue`]); cleared once the
        // re-issued work reaches a fold boundary. While set, steering that
        // arrives during the re-issued call takes the ordinary boundary fold
        // instead of abandoning another call — which is what makes an
        // abort→re-issue→abort spin impossible no matter how fast the user
        // types.
        let mut steering_reissued = false;

        'outer: loop {
            if self.tool_loop_break_requested {
                // Mirrors the sync loop: the repetition guard is a real
                // non-convergent stop, so it carries a marker, a closer, and a
                // visible notice instead of ending the turn silently.
                let error = RuntimeError::new("turn hit the tool-repetition guard");
                self.clear_empty_retry_reminder(empty_retries);
                self.record_turn_failed(iterations, &error);
                budget_exhausted = Some(BudgetExhausted::ToolRepetition);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::ToolRepetition,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(StreamingTurnError::runtime)?;
                let _ = render_tx
                    .send(RenderBlock::System {
                        id: id_gen.next(),
                        level: SystemLevel::Warn,
                        text: budget_exhausted_notice(
                            BudgetExhausted::ToolRepetition,
                            iterations,
                        ),
                    })
                    .await;
                break 'outer;
            }
            iterations += 1;
            if iterations > self.max_iterations {
                let error = RuntimeError::new(
                    "conversation loop exceeded the maximum number of iterations",
                );
                self.clear_empty_retry_reminder(empty_retries);
                self.record_turn_failed(iterations, &error);
                // Budget exhausted, not failed: this boundary is only reached
                // after a prior iteration closed the session well-formed
                // (assistant `tool_use` + user tool-results), so preserve the
                // work instead of rolling back. Append a synthetic closer, warn
                // on the render channel, and end the turn Ok(..) with the budget
                // marker so the caller (or user) can continue in a follow-up.
                budget_exhausted = Some(BudgetExhausted::Iterations);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::Iterations,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(StreamingTurnError::runtime)?;
                // Streaming notice (sync path uses `eprintln`). Inlined — not a
                // `&self` helper — so the send future never holds `&self` across
                // the await, which the spawned turn's `Send` bound forbids.
                let _ = render_tx
                    .send(RenderBlock::System {
                        id: id_gen.next(),
                        level: SystemLevel::Warn,
                        text: budget_exhausted_notice(BudgetExhausted::Iterations, iterations),
                    })
                    .await;
                break 'outer;
            }
            // The last iterations ask for the report; the final one forbids
            // tools, so a capped run ends on the model's answer (budget_wrap_up).
            let wrap_up_choice = self.arm_budget_wrap_up(iterations);

            // Wall-clock budget: a spawned sub-agent bounds a straggler, and the
            // interactive host sets `now + turn budget` each turn as the runaway
            // circuit breaker. Checked at the iteration boundary — cooperative,
            // but the per-chunk stream idle timeout bounds the in-flight read so
            // this check is always reached. Same as the iteration cap above:
            // preserve the well-formed work and end Ok(..) with the budget marker.
            if self
                .deadline
                .is_some_and(|d| std::time::Instant::now() >= d)
            {
                // Progress-gated extension: a turn that is demonstrably still
                // externalizing progress (fresh successful edit/write/plan
                // results since the last window) earns a bounded deadline push
                // instead of a mid-work stop — the legitimate long audit or
                // deploy pipeline the blunt 60-minute cut kept interrupting.
                // Fake-progress grinds are still bounded: the extension count
                // is capped, reads/probes don't count as progress, and the
                // cross-turn escalation ladder catches the repeat pattern.
                let extension_granted = match self.deadline_extension {
                    Some((max_extensions, step))
                        if deadline_extensions_used < max_extensions
                            && super::count_progress_tool_results(&tool_results)
                                > extension_progress_marker =>
                    {
                        deadline_extensions_used += 1;
                        extension_progress_marker =
                            super::count_progress_tool_results(&tool_results);
                        self.deadline = Some(std::time::Instant::now() + step);
                        let _ = render_tx
                            .send(RenderBlock::System {
                                id: id_gen.next(),
                                level: SystemLevel::Info,
                                text: format!(
                                    "[budget] deadline extended +{}m ({deadline_extensions_used}/{max_extensions}) — fresh progress detected",
                                    step.as_secs() / 60
                                ),
                            })
                            .await;
                        true
                    }
                    _ => false,
                };
                if !extension_granted {
                    let error = RuntimeError::new("agent exceeded its time budget");
                    self.clear_empty_retry_reminder(empty_retries);
                    self.record_turn_failed(iterations, &error);
                    budget_exhausted = Some(BudgetExhausted::Deadline);
                    self.push_budget_exhausted_closer(
                        BudgetExhausted::Deadline,
                        iterations,
                        &mut assistant_messages,
                    )
                    .map_err(StreamingTurnError::runtime)?;
                    let _ = render_tx
                        .send(RenderBlock::System {
                            id: id_gen.next(),
                            level: SystemLevel::Warn,
                            text: budget_exhausted_notice(BudgetExhausted::Deadline, iterations),
                        })
                        .await;
                    break 'outer;
                }
            }

            // Output-token budget (cost circuit breaker): the in-turn output
            // delta from turn start. Bounds an agentic loop that keeps generating
            // (re-planning an unachievable goal, re-invoking Workflow/agents)
            // without converging — the multi-day-runaway case the iteration cap
            // misses when a few iterations each fan out huge multi-agent work.
            if self.turn_output_token_budget.is_some_and(|budget| {
                self.usage_tracker
                    .cumulative_usage()
                    .output_tokens
                    .saturating_sub(turn_start_output_tokens)
                    > budget
            }) {
                let error = RuntimeError::new("turn exceeded its output-token budget");
                self.clear_empty_retry_reminder(empty_retries);
                self.record_turn_failed(iterations, &error);
                budget_exhausted = Some(BudgetExhausted::OutputTokens);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::OutputTokens,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(StreamingTurnError::runtime)?;
                let _ = render_tx
                    .send(RenderBlock::System {
                        id: id_gen.next(),
                        level: SystemLevel::Warn,
                        text: budget_exhausted_notice(BudgetExhausted::OutputTokens, iterations),
                    })
                    .await;
                break 'outer;
            }

            // Input-token budget (cache-miss cost circuit breaker): the
            // in-turn full-price-input delta from turn start. Bounds a
            // cache-dead loop that re-sends the whole transcript uncached on
            // every call — millions of input tokens with little output,
            // invisible to the output breaker above and comfortably inside
            // the wall clock.
            if self.turn_input_token_budget.is_some_and(|budget| {
                self.usage_tracker
                    .cumulative_usage()
                    .input_tokens
                    .saturating_sub(turn_start_input_tokens)
                    > budget
            }) {
                let error = RuntimeError::new("turn exceeded its input-token budget");
                self.clear_empty_retry_reminder(empty_retries);
                self.record_turn_failed(iterations, &error);
                budget_exhausted = Some(BudgetExhausted::InputTokens);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::InputTokens,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(StreamingTurnError::runtime)?;
                let _ = render_tx
                    .send(RenderBlock::System {
                        id: id_gen.next(),
                        level: SystemLevel::Warn,
                        text: budget_exhausted_notice(BudgetExhausted::InputTokens, iterations),
                    })
                    .await;
                break 'outer;
            }

            if render_tx.is_closed() {
                self.clear_empty_retry_reminder(empty_retries);
                return Err(self.cancel_streaming_turn(
                    iterations,
                    "render channel closed before request",
                    message_count_before,
                ));
            }

            // This turn pre-armed onto the quota fallback from the session
            // cooldown: announce it once, before the first request, so the user
            // knows why the model differs without re-spending the main model's
            // retry budget. Consumed here so it fires exactly once per turn.
            if self.quota_prearm_notice_pending {
                self.quota_prearm_notice_pending = false;
                if let Some((_, model)) = self.quota_fallback_client.clone() {
                    let text = self.quota_prearm_notice_text(&model);
                    let _ = render_tx
                        .send(RenderBlock::System {
                            id: id_gen.next(),
                            level: SystemLevel::Info,
                            text,
                        })
                        .await;
                }
            }
            if self.refusal_prearm_notice_pending {
                self.refusal_prearm_notice_pending = false;
                let _ = render_tx
                    .send(RenderBlock::System {
                        id: id_gen.next(),
                        level: SystemLevel::Warn,
                        text: REFUSAL_DRY_PREARM_WARN.to_string(),
                    })
                    .await;
            }

            // Proactive compaction: compact *before* building the request.
            // The first iteration must use a local request estimate rather than
            // stale provider usage from the previous turn; later iterations use
            // fresh provider usage plus the local estimate. This prevents a
            // resumed or fast-growing session from jumping past the 85% trigger
            // before the runtime has a chance to shrink it.
            if iterations == 1 {
                if let Some(event) = self
                    .maybe_microcompact_preflight_streaming(&render_tx, &id_gen)
                    .await
                {
                    microcompact.get_or_insert(event);
                }
                if let Some(event) = self
                    .maybe_auto_compact_preflight_streaming(&render_tx, &id_gen)
                    .await
                {
                    auto_compaction.get_or_insert(event);
                } else {
                    self.maybe_state_distill_preflight();
                    self.maybe_precompaction_warn_streaming(&render_tx, &id_gen)
                        .await;
                }
            } else {
                if let Some(event) = self.maybe_microcompact_streaming(&render_tx, &id_gen).await {
                    microcompact.get_or_insert(event);
                }
                if let Some(event) = self.maybe_auto_compact_streaming(&render_tx, &id_gen).await {
                    auto_compaction.get_or_insert(event);
                } else {
                    self.maybe_state_distill();
                    self.maybe_precompaction_warn_streaming(&render_tx, &id_gen)
                        .await;
                }
            }

            // Assemble the request without re-running the synchronous overflow
            // guard: the async preflight/iteration compaction just above already
            // shrank the session to fit (it owns the same emergency
            // context-window seam), so calling the guard here would at best
            // duplicate that work and at worst make a *second* blocking LLM
            // summary round-trip on this very `select!` thread — freezing the
            // spinner/stream for the whole summary (the reported first-turn
            // freeze). `assemble_request` is pure (Arc clones only), so it is
            // safe to run every iteration on the render thread.
            let __req_t = std::time::Instant::now();
            let mut stream_stamps = crate::request_timings::StreamStamps::begun(__req_t);
            // Recall runs in `spawn_blocking` (FREEZE-1): the dense ONNX embedding
            // forward pass no longer executes on this `select!` task, so it cannot
            // starve render_tick. The recall inputs are cloned out of `&self`
            // here, BEFORE the await, so the spawned turn future never holds a
            // `&self` borrow across it (the runtime is `Send` but not `Sync`).
            // `assemble_request` itself is pure (Arc clones only).
            let recall_section = Self::recall_reminder_section(
                self.memory_retriever.clone(),
                self.recall_query_text().map(std::borrow::Cow::into_owned),
                self.session_tracer.clone(),
                self.recall_seat.clone(),
            )
            .await;
            let recall_attached = !recall_section.is_empty();
            let mut wire_reminders = self.transient_reminders.clone();
            wire_reminders.extend(recall_section);
            // Persist the reminder set as a trailing System transcript message
            // (deduped across iterations) instead of riding the newest user
            // wire message — the wire-only injection changed the previous
            // request's tail bytes every iteration, re-billing the prior tool
            // batch as uncached input. Runs after the compaction preflights so
            // a freshly-persisted reminder is never immediately compacted away.
            // Rollback target for a request that never reaches the provider, read
            // AFTER the compaction preflight above and BEFORE the absorb below.
            // `message_count_before` (turn start) cannot be used here: compaction
            // swaps in a summarized, usually SHORTER session, so truncating to a
            // turn-start index is a no-op on exactly the long sessions that
            // overflow — leaving the undelivered user message in the transcript.
            // The two stream-failure paths further down still use
            // `message_count_before`; they are pre-existing behavior and are
            // deliberately left alone by this repair.
            let undispatched_from = self.session.messages.len();
            self.absorb_wire_reminders_into_session(&wire_reminders);
            if let Err(error) = self.ensure_request_context_budget(&[]) {
                self.clear_empty_retry_reminder(empty_retries);
                self.record_turn_failed(iterations, &error);
                // Always drop the reminder message the absorb may have appended
                // for this undispatched request — `clear_empty_retry_reminder`
                // only touches the transient list, never the transcript. On the
                // first iteration the orphaned user message goes with it; on a
                // later one the tail is delivered work that must survive.
                let rollback_to = if iterations == 1 {
                    undispatched_from.saturating_sub(1)
                } else {
                    undispatched_from
                };
                self.withdraw_undispatched_reminders(rollback_to);
                return Err(StreamingTurnError::from(error));
            }
            let request = self.assemble_request(Arc::from(Vec::new()), wrap_up_choice);
            // The compaction/resume status reminder has now ridden this request;
            // drop it so it does not re-instruct `session_recall` on every later
            // turn (a re-orientation loop). A fresh compaction re-seeds it, so it
            // still surfaces once per compaction event.
            self.drop_compaction_status_reminder();
            if __req_t.elapsed().as_millis() >= 50 && crate::turn_profiling_enabled() {
                eprintln!(
                    "[TURN-SEG] request_system_prompt+assemble = {}ms (recall off-thread)",
                    __req_t.elapsed().as_millis()
                );
            }
            let text_block_id = id_gen.next();
            // The live-ledger fact the footer keys on: which model this request
            // goes out on, and why — announced before the stream opens, so a
            // swap (refusal fallback, quota fallback, a deep-gate leg) shows as
            // it happens instead of after the reply. Each swap's explanation is
            // already a `System` row above; this adds no transcript row.
            if let Some((model, source)) = self.wire_model() {
                let _ = render_tx
                    .send(RenderBlock::WireModel(WireModel {
                        model: model.to_string(),
                        source,
                    }))
                    .await;
            }
            let events_for_build: Vec<AssistantEvent> =
                if let Some(async_client) = self.active_async_client() {
                    // Live async path (L7c seam) with retry on transient errors.
                    // `active_async_client` routes through the cross-provider
                    // quota fallback for the rest of the turn once one has armed,
                    // so a native sync-only session (no `async_api_client`) still
                    // reaches this branch after a quota swap.
                    // The hook abort flag cuts a transient-error backoff short so
                    // a foreground Ctrl+C is observed during the wait, not only at
                    // the next iteration boundary (WI-G).
                    let async_client = async_client.clone();
                    // Gen-abort probe. The call's render blocks travel through a
                    // bounded interposer channel instead of straight to
                    // `render_tx` so this task can watch the stream go by (see
                    // [`race_stream_against_steering`]). Same capacity and the
                    // same awaited forwarding as before, so backpressure is
                    // unchanged; retry notices ride the same channel so they
                    // cannot overtake blocks still queued ahead of them.
                    let (probe_tx, mut probe_rx) =
                        mpsc::channel::<RenderBlock>(DEFAULT_STREAMING_CHANNEL_CAPACITY);
                    let notice_tx = probe_tx.clone();
                    let notice_ids = id_gen.clone();
                    // The model this turn streams on, cloned out before the
                    // closures so a foreground 429 can feed the SAME per-provider
                    // cool-down state (`api::quota`) the sub-agent admission path
                    // reads — the foreground main turn used to ride out the window
                    // here without ever recording the throttle (quota-view/router
                    // gap).
                    let rate_limit_model = self
                        .rate_limit_model_for_active_stream()
                        .map(str::to_string);
                    // How long this stream argues with a capacity wall. A
                    // deep-gate VERIFY leg answers first and keeps its ranked
                    // walk's cap; outside one the main turn asks whether it has
                    // an escape at all, because `decide_quota_escape` below only
                    // gets to look once this call gives up (t-5499: a 429 with no
                    // reset hint burned the whole 300 s account budget in front
                    // of a fallback that answered in seconds).
                    let rate_limit_retry_cap = self
                        .verifier_rate_limit_retry_cap()
                        .or_else(|| self.main_turn_rate_limit_retry_cap());
                    let mut stream_fut = Box::pin(crate::retry::retry_async(
                        "stream_async",
                        Some(self.hook_abort_signal.flag()),
                        rate_limit_retry_cap,
                        move |attempt, error: &RuntimeError| {
                            if let Some(model) = rate_limit_model.as_deref() {
                                crate::retry::mark_foreground_capacity_stall(
                                    model,
                                    &error.to_string(),
                                    attempt,
                                );
                            }
                        },
                        move |attempt, delay, error: &RuntimeError| {
                            // Keep the live UI honest while L2 backs off: a
                            // capacity stall (overload/429/5xx) is otherwise a
                            // silent multi-second pause before the next attempt,
                            // which reads as a freeze. Mirrors the api-level (L0)
                            // retry notice, but also fires for mid-stream errors
                            // the establish-time path never sees. The label
                            // vocabulary lives in `core_types::retry_signal` so
                            // it stays in lockstep with the backoff classifier.
                            let error_text = error.to_string();
                            let label = core_types::retry_signal::retry_notice_label(&error_text);
                            // The cause rides along, clipped to one line: a bare
                            // "rate limited" reads as a transient stall, while
                            // Google's RESOURCE_EXHAUSTED "check quota" is a
                            // quota the person has to know about (live report
                            // 2026-09-02: gemini-3.6-flash looked stuck for
                            // minutes on a free tier that was simply used up).
                            let mut reason: String = error_text
                                .chars()
                                .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
                                .take(120)
                                .collect();
                            if error_text.chars().count() > 120 {
                                reason.push('…');
                            }
                            let _ = notice_tx.try_send(RenderBlock::System {
                                id: notice_ids.next(),
                                level: SystemLevel::Warn,
                                text: format!(
                                    "{label}; retrying in {}s (attempt {attempt}) — {reason}",
                                    delay.as_secs().max(1)
                                ),
                            });
                            let _ = notice_tx.try_send(RenderBlock::StreamPhase(
                                crate::message_stream::types::StreamPhase::Retrying {
                                    attempt,
                                    delay_secs: delay.as_secs().max(1),
                                },
                            ));
                        },
                        move |attempt| {
                            let req = request.clone();
                            let tx = probe_tx.clone();
                            let client = async_client.clone();
                            async move {
                                // The wait for the first token starts here, on
                                // every attempt — the live status line counts
                                // from this block until content arrives. The
                                // factory's `attempt` counts prior failures;
                                // the phase names the attempt itself, 1-based
                                // like the retry notice.
                                let _ = tx.try_send(RenderBlock::StreamPhase(
                                    crate::message_stream::types::StreamPhase::RequestSent {
                                        attempt: attempt.saturating_add(1),
                                    },
                                ));
                                client.stream_async(req, tx, text_block_id).await
                            }
                        },
                    ));
                    let race = race_stream_against_steering(
                        &mut stream_fut,
                        &mut probe_rx,
                        &render_tx,
                        &self.steering,
                        self.hook_abort_signal.flag(),
                        !steering_reissued,
                        &mut stream_stamps,
                    )
                    .await;
                    // Dropping the future is what actually tears the upstream
                    // connection down on the re-issue path, and it is also what
                    // releases the last probe sender so the drain below can end.
                    // Explicit because `Box` has a `Drop`, which would otherwise
                    // hold this scope's borrow of `self` past the `&mut self`
                    // work in both arms.
                    drop(stream_fut);
                    let stream_result = match race {
                        StreamRace::Completed(outcome) => {
                            // Blocks the provider produced before the race
                            // resolved but that had not been forwarded yet. Best
                            // effort: if the render consumer is gone the stream
                            // already failed for that reason. The timing ledger
                            // reads them too: a stream fast enough to resolve
                            // before its first block was observed still had a
                            // first byte — at completion, not never.
                            while let Some(block) = probe_rx.recv().await {
                                stream_stamps.observe(&block);
                                let _ = render_tx.send(block).await;
                            }
                            outcome
                        }
                        StreamRace::SteeringReissue { open_text_block } => {
                            // Nothing from the abandoned call is settled: no
                            // assistant message, no iteration record, no
                            // retry/refusal/quota/deep-gate accounting — the
                            // branch returns before every one of those.
                            //
                            // Usage is the one honest gap. The provider DOES
                            // bill the partial generation, but the usage event
                            // arrives at the end of a stream we are dropping, so
                            // there is nothing to record: `usage_tracker` and
                            // the two turn token breakers under-count by one
                            // abandoned call per fold boundary. Compaction is
                            // unaffected (`effective_context_tokens` maxes in a
                            // local estimate); only the cost view drifts low.
                            //
                            // The deltas already on screen stay there (same as
                            // the refusal path) — close their block so the
                            // re-issued call, which takes a fresh block id,
                            // opens cleanly.
                            if let Some(id) = open_text_block {
                                let _ = render_tx
                                    .send(RenderBlock::TextDelta {
                                        id,
                                        text: String::new(),
                                        done: true,
                                    })
                                    .await;
                            }
                            drop(probe_rx);
                            steering_reissued = true;
                            let steers = self.drain_steering();
                            for steer in &steers {
                                let _ = render_tx
                                    .send(RenderBlock::System {
                                        id: id_gen.next(),
                                        level: SystemLevel::Info,
                                        text: format!("{STEERING_ECHO_PREFIX}{steer}"),
                                    })
                                    .await;
                            }
                            self.fold_steering_into_settled_tail(&steers)
                                .map_err(StreamingTurnError::from)?;
                            // Attested AFTER the fold, not at the race's
                            // decision: a firing has to mean the steer really
                            // reached the next request, and the fold is where
                            // that becomes true (a fold error returns above and
                            // records nothing, which is the honest reading —
                            // the call was abandoned for nothing).
                            //
                            // The steer COUNT is deliberately not carried: the
                            // ledger is a counter table whose reasons are
                            // `&'static str` by construction, and per-call
                            // detail is an explicit non-goal there (see the
                            // `harness_attest` module doc). One firing per
                            // re-issued call is the axis this proves.
                            telemetry::attest_fired(telemetry::HarnessFeature::SteeringReissue);
                            // Re-request from the top with the identical
                            // thinking/effort/tool configuration — the request is
                            // rebuilt by the same `assemble_request` call every
                            // other iteration uses, so the re-issue is a pure
                            // append to the cached prefix rather than a settings
                            // delta that would invalidate the whole prompt cache.
                            // Costing an iteration is the established idiom for a
                            // re-requested call here (quota hold/swap, tier
                            // demotion, refusal retry, empty-stream retry all
                            // `continue 'outer` the same way).
                            continue 'outer;
                        }
                    };
                    match stream_result {
                        Ok(events) => events,
                        Err(error) => {
                            if let Some(cancelled) = self.cancel_streaming_turn_if_aborted(
                                iterations,
                                "turn stopped by abort signal",
                            ) {
                                self.clear_empty_retry_reminder(empty_retries);
                                return Err(cancelled);
                            }
                            if !provider_overflow_recovery_attempted
                                && error.provider_error_class()
                                    == Some(crate::ProviderErrorClass::ContextOverflow)
                            {
                                provider_overflow_recovery_attempted = true;
                                // Learn the wire's real ceiling before compacting
                                // (see `adopt_provider_context_ceiling`): this
                                // session's thresholds are otherwise derived from
                                // a window the provider does not honor, and the
                                // next turn rebuilds to the same oversized shape.
                                if let Some(ceiling) =
                                    ::api::context_overflow_ceiling_tokens(&error.to_string())
                                {
                                    self.adopt_provider_context_ceiling(ceiling);
                                }
                                if let Some(event) = self
                                    .recover_provider_context_overflow_streaming(
                                        &render_tx, &id_gen,
                                    )
                                    .await
                                {
                                    auto_compaction.get_or_insert(event);
                                    continue 'outer;
                                }
                            }
                            // Main model's quota window is exhausted (RateLimit
                            // survived the retry budget): HOLD on the main model
                            // when its window lifts within the wait band, else
                            // swap to the cross-provider fallback — re-requesting
                            // this turn either way rather than killing it.
                            match self.decide_quota_escape(&error) {
                                QuotaEscape::Wait(wait) => {
                                    let model = self.context_model.clone().unwrap_or_default();
                                    let _ = render_tx
                                        .send(RenderBlock::System {
                                            id: id_gen.next(),
                                            level: SystemLevel::Warn,
                                            text: quota_wait_hold_warn(&model, wait),
                                        })
                                        .await;
                                    tokio::time::sleep(wait).await;
                                    continue 'outer;
                                }
                                QuotaEscape::Fallback(model) => {
                                    let _ = render_tx
                                        .send(RenderBlock::System {
                                            id: id_gen.next(),
                                            level: SystemLevel::Warn,
                                            text: quota_fallback_swap_warn(&model),
                                        })
                                        .await;
                                    continue 'outer;
                                }
                                // The provider shed this tier: re-request the same
                                // turn one tier down, same provider. The override is
                                // already recorded, so the retry picks it up through
                                // `assemble_request`.
                                QuotaEscape::Lighter(model) => {
                                    let shed = self
                                        .context_model
                                        .clone()
                                        .unwrap_or_else(|| "the main model".to_string());
                                    let _ = render_tx
                                        .send(RenderBlock::System {
                                            id: id_gen.next(),
                                            level: SystemLevel::Warn,
                                            text: overload_demotion_warn(&shed, &model),
                                        })
                                        .await;
                                    continue 'outer;
                                }
                                QuotaEscape::None => {}
                            }
                            if self
                                .answer_gateway_refusal(
                                    &error,
                                    recall_attached,
                                    undispatched_from,
                                    &render_tx,
                                    &id_gen,
                                )
                                .await
                            {
                                continue 'outer;
                            }
                            // The NETWORK failed, not the provider: wait for
                            // the link to come back and re-run this call, for
                            // as long as a person would wait ("인터넷이 끊겨도
                            // 연결되면 자동으로 이어가야함", 2026-09-02). The
                            // answer that was cut off restarts; nothing of it
                            // ran, so nothing is done twice.
                            if let Some(host) =
                                core_types::retry_signal::network_outage_host(&error.to_string())
                            {
                                if self.await_network_return(&render_tx, &id_gen, &host).await {
                                    continue 'outer;
                                }
                            }
                            self.clear_empty_retry_reminder(empty_retries);
                            self.record_turn_failed(iterations, &error);
                            if iterations == 1 {
                                Arc::make_mut(&mut self.session.messages)
                                    .truncate(message_count_before);
                                self.session.mark_transcript_dirty();
                            }
                            return Err(StreamingTurnError::from(error));
                        }
                    }
                } else {
                    // Default legacy path: synchronous collect-then-replay.
                    let events = match self.api_client.stream(request) {
                        Ok(events) => events,
                        Err(error) => {
                            if let Some(cancelled) = self.cancel_streaming_turn_if_aborted(
                                iterations,
                                "turn stopped by abort signal",
                            ) {
                                self.clear_empty_retry_reminder(empty_retries);
                                return Err(cancelled);
                            }
                            if !provider_overflow_recovery_attempted
                                && error.provider_error_class()
                                    == Some(crate::ProviderErrorClass::ContextOverflow)
                            {
                                provider_overflow_recovery_attempted = true;
                                // Learn the wire's real ceiling before compacting
                                // (see `adopt_provider_context_ceiling`): this
                                // session's thresholds are otherwise derived from
                                // a window the provider does not honor, and the
                                // next turn rebuilds to the same oversized shape.
                                if let Some(ceiling) =
                                    ::api::context_overflow_ceiling_tokens(&error.to_string())
                                {
                                    self.adopt_provider_context_ceiling(ceiling);
                                }
                                if let Some(event) = self
                                    .recover_provider_context_overflow_streaming(
                                        &render_tx, &id_gen,
                                    )
                                    .await
                                {
                                    auto_compaction.get_or_insert(event);
                                    continue 'outer;
                                }
                            }
                            // Same escape as the async branch: a native sync-only
                            // session either holds on the main model (wait band)
                            // or swaps to the cross-provider fallback (reached
                            // next iteration via `active_async_client`).
                            match self.decide_quota_escape(&error) {
                                QuotaEscape::Wait(wait) => {
                                    let model = self.context_model.clone().unwrap_or_default();
                                    let _ = render_tx
                                        .send(RenderBlock::System {
                                            id: id_gen.next(),
                                            level: SystemLevel::Warn,
                                            text: quota_wait_hold_warn(&model, wait),
                                        })
                                        .await;
                                    tokio::time::sleep(wait).await;
                                    continue 'outer;
                                }
                                QuotaEscape::Fallback(model) => {
                                    let _ = render_tx
                                        .send(RenderBlock::System {
                                            id: id_gen.next(),
                                            level: SystemLevel::Warn,
                                            text: quota_fallback_swap_warn(&model),
                                        })
                                        .await;
                                    continue 'outer;
                                }
                                // The provider shed this tier: re-request the same
                                // turn one tier down, same provider. The override is
                                // already recorded, so the retry picks it up through
                                // `assemble_request`.
                                QuotaEscape::Lighter(model) => {
                                    let shed = self
                                        .context_model
                                        .clone()
                                        .unwrap_or_else(|| "the main model".to_string());
                                    let _ = render_tx
                                        .send(RenderBlock::System {
                                            id: id_gen.next(),
                                            level: SystemLevel::Warn,
                                            text: overload_demotion_warn(&shed, &model),
                                        })
                                        .await;
                                    continue 'outer;
                                }
                                QuotaEscape::None => {}
                            }
                            if self
                                .answer_gateway_refusal(
                                    &error,
                                    recall_attached,
                                    undispatched_from,
                                    &render_tx,
                                    &id_gen,
                                )
                                .await
                            {
                                continue 'outer;
                            }
                            // The NETWORK failed, not the provider: wait for
                            // the link to come back and re-run this call, for
                            // as long as a person would wait ("인터넷이 끊겨도
                            // 연결되면 자동으로 이어가야함", 2026-09-02). The
                            // answer that was cut off restarts; nothing of it
                            // ran, so nothing is done twice.
                            if let Some(host) =
                                core_types::retry_signal::network_outage_host(&error.to_string())
                            {
                                if self.await_network_return(&render_tx, &id_gen, &host).await {
                                    continue 'outer;
                                }
                            }
                            self.clear_empty_retry_reminder(empty_retries);
                            self.record_turn_failed(iterations, &error);
                            if iterations == 1 {
                                Arc::make_mut(&mut self.session.messages)
                                    .truncate(message_count_before);
                                self.session.mark_transcript_dirty();
                            }
                            return Err(StreamingTurnError::from(error));
                        }
                    };

                    // Replay events into RenderBlocks while preserving
                    // them for `build_assistant_message`, which remains
                    // the source of truth for the session bookkeeping.
                    let mut text_emitted = false;
                    for event in &events {
                        if let AssistantEvent::TextDelta(delta) = &event {
                            text_emitted = true;
                            if render_tx
                                .send(RenderBlock::TextDelta {
                                    id: text_block_id,
                                    text: delta.clone(),
                                    done: false,
                                })
                                .await
                                .is_err()
                            {
                                self.clear_empty_retry_reminder(empty_retries);
                                return Err(self.cancel_streaming_turn(
                                    iterations,
                                    "render channel closed during text streaming",
                                    message_count_before,
                                ));
                            }
                        }
                    }
                    if text_emitted
                        && render_tx
                            .send(RenderBlock::TextDelta {
                                id: text_block_id,
                                text: String::new(),
                                done: true,
                            })
                            .await
                            .is_err()
                    {
                        self.clear_empty_retry_reminder(empty_retries);
                        return Err(self.cancel_streaming_turn(
                            iterations,
                            "render channel closed finalizing text",
                            message_count_before,
                        ));
                    }
                    events
                };

            let __ba_t = std::time::Instant::now();
            let __ba_result =
                build_assistant_message(normalize_empty_assistant_stream(events_for_build));
            if __ba_t.elapsed().as_millis() >= 50 && crate::turn_profiling_enabled() {
                eprintln!(
                    "[TURN-SEG] build_assistant_message = {}ms (synchronous; starves render_tick)",
                    __ba_t.elapsed().as_millis()
                );
            }
            // Anthropic safety-classifier refusal (`stop_reason: "refusal"`):
            // drop the refused partial (never pushed to history — it stays on
            // screen as the already-streamed deltas, but the retry renders under
            // a fresh block id) and either retry once on Opus 4.8 (Fable/Mythos)
            // with a warn line, or surface a notice and end (already fell back,
            // or a non-Fable model). Anthropic-only; a non-Anthropic model yields
            // `Proceed` and falls through unchanged.
            if is_refusal_stop_reason(__ba_result.stop_reason().unwrap_or_default()) {
                let refused_usage = __ba_result.usage();
                match self.decide_refusal_fallback() {
                    decision @ (RefusalDecision::Retry | RefusalDecision::RetrySameModel) => {
                        if let Some(usage) = refused_usage {
                            self.usage_tracker.record(usage);
                        }
                        let text = if matches!(decision, RefusalDecision::Retry) {
                            REFUSAL_FALLBACK_WARN
                        } else {
                            super::fallback::REFUSAL_SAME_MODEL_RETRY_WARN
                        };
                        let _ = render_tx
                            .send(RenderBlock::System {
                                id: id_gen.next(),
                                level: SystemLevel::Warn,
                                text: text.to_string(),
                            })
                            .await;
                        continue 'outer;
                    }
                    RefusalDecision::Surface => {
                        if let Some(usage) = refused_usage {
                            self.usage_tracker.record(usage);
                        }
                        let _ = render_tx
                            .send(RenderBlock::System {
                                id: id_gen.next(),
                                level: SystemLevel::Warn,
                                text: REFUSAL_SURFACED_NOTICE.to_string(),
                            })
                            .await;
                        let assistant_message = refusal_surfaced_message();
                        self.record_assistant_iteration(iterations, &assistant_message, 0);
                        self.session
                            .push_message(assistant_message)
                            .map_err(|error| StreamingTurnError::runtime(error.to_string()))?;
                        if let Some(msg) = self.session.messages.last().cloned() {
                            assistant_messages.push(msg);
                        }
                        break 'outer;
                    }
                    RefusalDecision::Proceed => {}
                }
            }
            let (assistant_message, usage, turn_prompt_cache_events, stop_reason) =
                match __ba_result {
                    AssistantTurn::Content {
                        message,
                        usage,
                        prompt_cache_events,
                        stop_reason,
                    } => self.accept_assistant_content_turn(
                        &mut empty_retries,
                        &mut empty_recovery_attempted,
                        message,
                        usage,
                        prompt_cache_events,
                        stop_reason,
                    ),
                    // Clean stop with no content (thinking-only / transient
                    // empty): record telemetry and re-request a bounded number
                    // of times before failing, so a one-off empty completion
                    // doesn't discard the turn's work.
                    AssistantTurn::Empty { usage, stop_reason } => {
                        match self.handle_empty_assistant_turn(
                            usage,
                            stop_reason.as_deref(),
                            &mut empty_retries,
                            &mut empty_recovery_attempted,
                        ) {
                            EmptyAssistantAction::Retry | EmptyAssistantAction::ContinueOnce => {
                                continue 'outer;
                            }
                            EmptyAssistantAction::Exhausted => {}
                        }
                        let assistant_message = empty_stream_exhausted_message();
                        if render_tx
                            .send(RenderBlock::TextDelta {
                                id: text_block_id,
                                text: EMPTY_STREAM_EXHAUSTED_FALLBACK_TEXT.to_string(),
                                done: false,
                            })
                            .await
                            .is_err()
                        {
                            return Err(self.cancel_streaming_turn(
                                iterations,
                                "render channel closed delivering empty-response fallback",
                                message_count_before,
                            ));
                        }
                        if render_tx
                            .send(RenderBlock::TextDelta {
                                id: text_block_id,
                                text: String::new(),
                                done: true,
                            })
                            .await
                            .is_err()
                        {
                            return Err(self.cancel_streaming_turn(
                                iterations,
                                "render channel closed finalizing empty-response fallback",
                                message_count_before,
                            ));
                        }
                        self.record_assistant_iteration(iterations, &assistant_message, 0);
                        self.session
                            .push_message(assistant_message)
                            .map_err(|error| StreamingTurnError::runtime(error.to_string()))?;
                        if let Some(msg) = self.session.messages.last().cloned() {
                            assistant_messages.push(msg);
                        }
                        break 'outer;
                    }
                };
            if let Some(usage) = usage {
                self.usage_tracker.record(usage);
            }
            // Forward a real usage snapshot so the HUD shows accurate ctx/cost
            // mid-turn instead of a char-count estimate. Non-critical telemetry,
            // so a closed render channel is ignored (the turn is unwinding anyway).
            //
            // `ctx_tokens` is the *provider's* count of what occupied the
            // context window on the latest request (input + cache read/write),
            // not a chars/4 transcript guess. When the latest turn carries no
            // usage yet (e.g. a tool-only iteration before the first billed
            // response), fall back to the local estimate so the ledger still
            // advances instead of snapping to zero.
            let ctx_tokens = {
                let provider_ctx = self.usage_tracker.current_turn_usage().context_tokens();
                if provider_ctx > 0 {
                    u64::from(provider_ctx)
                } else {
                    u64::try_from(self.estimated_tokens()).unwrap_or(u64::MAX)
                }
            };
            let _ = render_tx
                .send(RenderBlock::Usage {
                    ctx_tokens,
                    cumulative: self.usage_tracker.cumulative_usage(),
                    current: self.usage_tracker.current_turn_usage(),
                })
                .await;
            // Surface cache-efficiency warnings live. The low-cache-hit streak
            // warning previously reached only the headless JSON event surface —
            // an interactive session re-billing its whole transcript uncached
            // every call (the leak class the input-token breaker also guards)
            // burned for hours with a working smoke detector and no bell.
            // One line per streak (the record layer edge-triggers it), Warn
            // level, non-critical: a closed render channel is ignored.
            for event in &turn_prompt_cache_events {
                if let Some(warning) = &event.warning {
                    let _ = render_tx
                        .send(RenderBlock::System {
                            id: id_gen.next(),
                            level: SystemLevel::Warn,
                            text: format!("[cache] {warning}"),
                        })
                        .await;
                }
            }
            self.record_prompt_cache_breaks(iterations, &turn_prompt_cache_events);
            self.record_request_timing(iterations, &stream_stamps, "completed");
            prompt_cache_events.extend(turn_prompt_cache_events);
            let pending_tool_use_count = assistant_message
                .blocks
                .iter()
                .filter(|block| matches!(block, ContentBlock::ToolUse { .. }))
                .count();
            self.record_assistant_iteration(iterations, &assistant_message, pending_tool_use_count);

            if pending_tool_use_count == 0 {
                let final_visible_text = assistant_message
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                self.session
                    .push_message(assistant_message)
                    .map_err(|error| StreamingTurnError::runtime(error.to_string()))?;
                if let Some(msg) = self.session.messages.last().cloned() {
                    assistant_messages.push(msg);
                }
                // Text-only turn boundary (A). The tool-result drain below is
                // never reached on a turn the model answers with prose alone,
                // so steering typed during it would otherwise strand in the
                // queue until the *next* user turn. Instead, fold any pending
                // steering into a fresh user message and run one more
                // iteration: the last message is now this assistant reply, so a
                // new user turn is well-formed (no two consecutive user roles).
                //
                // The same fresh-user-turn mechanism also carries a truncation
                // continuation: if the provider cut this response off at the
                // output-token limit (so the model never reached the tool call
                // it was working toward), preserve the partial output and nudge
                // it to continue rather than ending the turn empty-handed.
                // Bounded so a model that keeps over-spending the window can't
                // loop forever.
                let steers = self.drain_steering();
                let truncated = take_truncation_continuation(
                    stop_reason.as_deref(),
                    &mut truncation_continuations,
                );
                if steers.is_empty() && !truncated {
                    // Turn-end gate: a reply that ends on a promise of undone
                    // work (or, on an autonomous surface, a question nobody
                    // can answer) gets a bounded re-prompt instead of ending
                    // the turn — the harness-enforced version of the prompt's
                    // "check your last paragraph" discipline.
                    if let Some(issue) = self.take_turn_end_gate_issue(
                        &final_visible_text,
                        &mut turn_end_gate_reprompts,
                    ) {
                        let _ = render_tx
                            .send(RenderBlock::System {
                                id: id_gen.next(),
                                level: SystemLevel::Info,
                                text: super::turn_end_gate::turn_end_gate_banner(issue)
                                    .to_string(),
                            })
                            .await;
                        self.session
                            .push_user_text(self.turn_end_gate_reminder_text(issue))
                            .map_err(|error| StreamingTurnError::runtime(error.to_string()))?;
                        continue;
                    }
                    // The receipt: this turn edited, then a check ran green
                    // after the last edit. The harness's own record, one line.
                    if let Some(receipt) = self.completion_receipt_line() {
                        let _ = render_tx
                            .send(RenderBlock::System {
                                id: id_gen.next(),
                                level: SystemLevel::Info,
                                text: receipt,
                            })
                            .await;
                    }
                    break 'outer;
                }
                let mut continuation_text = String::new();
                if truncated {
                    continuation_text.push_str(TRUNCATION_CONTINUATION_REMINDER);
                }
                for steer in steers {
                    let _ = render_tx
                        .send(RenderBlock::System {
                            id: id_gen.next(),
                            level: SystemLevel::Info,
                            text: format!("{STEERING_ECHO_PREFIX}{steer}"),
                        })
                        .await;
                    if !continuation_text.is_empty() {
                        continuation_text.push_str("\n\n");
                    }
                    continuation_text.push_str(&steering_message(&steer));
                }
                self.session
                    .push_user_text(continuation_text)
                    .map_err(|error| StreamingTurnError::runtime(error.to_string()))?;
                // A fold boundary was reached, so the gen-abort cap resets: the
                // next generation may again be abandoned for a *newer* steer.
                steering_reissued = false;
                continue;
            }
            if let Err(error) = self.check_tool_call_budget(tool_calls, pending_tool_use_count) {
                self.record_turn_failed(iterations, &error);
                // Tool-call budget: the over-budget assistant `tool_use` batch is
                // not yet in the session (it is pushed just below), so the
                // session still ends on the prior well-formed `user` message.
                // Drop the pending batch — no results will be produced for it —
                // append a closer, warn, and end Ok(..) with the marker so the
                // work already done survives instead of being rolled back.
                budget_exhausted = Some(BudgetExhausted::ToolCalls);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::ToolCalls,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(StreamingTurnError::runtime)?;
                let _ = render_tx
                    .send(RenderBlock::System {
                        id: id_gen.next(),
                        level: SystemLevel::Warn,
                        text: budget_exhausted_notice(BudgetExhausted::ToolCalls, iterations),
                    })
                    .await;
                break 'outer;
            }
            tool_calls += pending_tool_use_count;

            let tool_uses = collect_pending_tool_uses(&assistant_message);

            self.session
                .push_message(assistant_message)
                .map_err(|error| StreamingTurnError::runtime(error.to_string()))?;
            if let Some(msg) = self.session.messages.last().cloned() {
                assistant_messages.push(msg);
            }

            // ── Pass 1: Permission checks & pre-hooks (sequential) ──
            // Hooks and permission prompts require &mut self and may
            // await user input, so they must run one at a time.
            let mut prepared = Vec::with_capacity(tool_uses.len());
            for tool_use in &tool_uses {
                let tool_use_id = &tool_use.id;
                let tool_name = &tool_use.name;
                let input = &tool_use.input;
                let __hook_t = std::time::Instant::now();
                // Async seam: the blocking hook subprocess runs on a
                // `spawn_blocking` worker (see `run_pre_tool_use_hook_async`), so
                // this streaming task keeps servicing render/progress/cancel
                // polling while the hook runs instead of starving the render tick.
                let pre_hook_result = self.run_pre_tool_use_hook_async(tool_name, input).await;
                if __hook_t.elapsed().as_millis() >= 50 && crate::turn_profiling_enabled() {
                    eprintln!(
                        "[TURN-SEG] pre_tool_hook({tool_name}) = {}ms (off-task; render_tick free)",
                        __hook_t.elapsed().as_millis()
                    );
                }
                let effective_input = pre_hook_result
                    .updated_input()
                    .map_or_else(|| input.clone(), ToOwned::to_owned);
                let permission_context = PermissionContext::new(
                    pre_hook_result.permission_override(),
                    pre_hook_result.permission_reason().map(ToOwned::to_owned),
                );

                let permission_outcome = if let Some(outcome) =
                    pre_hook_denial_outcome(&pre_hook_result, tool_name)
                {
                    outcome
                } else if let Some(reason) = self.architect_edit_gate_denial(tool_name) {
                    // Architect contract backstop: a reserved foreground model
                    // editing directly is redirected to delegation before the
                    // ordinary permission policy runs.
                    PermissionOutcome::Deny { reason }
                } else {
                    let mut capture = CapturePrompter::new(PermissionPromptDecision::Allow);
                    let tentative = self.permission_policy.authorize_with_context(
                        tool_name,
                        &effective_input,
                        &permission_context,
                        Some(&mut capture),
                    );

                    match (capture.take(), tentative) {
                        (None, outcome) => outcome,
                        (Some(sync_request), PermissionOutcome::Allow) => {
                            let async_request = build_async_permission_request(&sync_request);
                            // Denial wording, not the prompt's ask wording — the
                            // request carries both precisely so a user's "no"
                            // reaches the model as a deterministic denial.
                            let reason_for_deny = sync_request
                                .deny_reason
                                .clone()
                                .or_else(|| sync_request.reason.clone());
                            let tool_label = tool_name.to_owned();
                            // The user is about to see a permission prompt:
                            // fire PermissionRequest plus Notification (the
                            // event Claude Code uses for OS-level pings).
                            self.fire_lifecycle_hook(
                                HookEvent::PermissionRequest,
                                &serde_json::json!({
                                    "tool_name": tool_name,
                                    "input": effective_input.as_str(),
                                    "reason": sync_request.reason,
                                }),
                            );
                            self.fire_lifecycle_hook(
                                HookEvent::Notification,
                                &serde_json::json!({
                                    "message": format!(
                                        "Zo needs permission to run `{tool_name}`"
                                    ),
                                    "tool_name": tool_name,
                                }),
                            );
                            // `None` = the prompt ceiling expired with nobody
                            // answering. Distinct from `Some(Deny)` (a real
                            // human "no") so the two get different model-facing
                            // text — see `permission_prompt_expired_reason`.
                            let budget = permission_prompt_timeout();
                            let awaited = match budget {
                                Some(budget) => {
                                    tokio::time::timeout(budget, prompter.decide(async_request))
                                        .await
                                        .ok()
                                }
                                None => Some(prompter.decide(async_request).await),
                            };
                            let decision = match awaited {
                                Some(Ok(decision)) => Some(decision),
                                Some(Err(error)) => {
                                    if let Some(cancelled) = self.cancel_streaming_turn_if_aborted(
                                        iterations,
                                        "permission prompt interrupted by abort signal",
                                    ) {
                                        return Err(cancelled);
                                    }
                                    self.record_turn_host_failure(
                                        iterations,
                                        "permission prompt abandoned",
                                    );
                                    Arc::make_mut(&mut self.session.messages)
                                        .truncate(message_count_before);
                                    self.session.mark_transcript_dirty();
                                    return Err(StreamingTurnError::Permission(error));
                                }
                                None => None,
                            };
                            match decision {
                                // "Always allow" grants a durable rule: it takes
                                // effect for the rest of this session and is
                                // queued for the host to persist to settings.
                                Some(AsyncPermissionDecision::Allow) => {
                                    self.permission_policy
                                        .grant_always(tool_name, &effective_input);
                                    PermissionOutcome::Allow
                                }
                                Some(AsyncPermissionDecision::AllowOnce) => PermissionOutcome::Allow,
                                Some(AsyncPermissionDecision::Deny) => PermissionOutcome::Deny {
                                    reason: reason_for_deny.unwrap_or_else(|| {
                                        format!("user denied tool '{tool_label}'")
                                    }),
                                },
                                None => PermissionOutcome::Deny {
                                    reason: permission_prompt_expired_reason(
                                        &tool_label,
                                        budget.unwrap_or_default(),
                                    ),
                                },
                            }
                        }
                        (Some(_), PermissionOutcome::Deny { reason }) => {
                            PermissionOutcome::Deny { reason }
                        }
                    }
                };
                if let PermissionOutcome::Deny { reason } = &permission_outcome {
                    self.fire_lifecycle_hook(
                        HookEvent::PermissionDenied,
                        &serde_json::json!({ "tool_name": tool_name, "reason": reason }),
                    );
                }

                let tool_block_id = id_gen.next();
                let tool_call_id = ToolCallId(tool_use_id.clone());
                if matches!(permission_outcome, PermissionOutcome::Allow)
                    && render_tx
                        .send(RenderBlock::ToolCall {
                            id: tool_block_id,
                            tool_call_id: tool_call_id.clone(),
                            name: tool_name.clone(),
                            summary: tool_summary_line(tool_name, &effective_input),
                            preview: tool_preview_from(tool_name, &effective_input),
                            status: ToolCallStatus::Running,
                        })
                        .await
                        .is_err()
                {
                    return Err(self.cancel_streaming_turn(
                        iterations,
                        "render channel closed before tool dispatch",
                        message_count_before,
                    ));
                }

                prepared.push(PreparedStreamingTool {
                    tool_use_id: tool_use_id.clone(),
                    tool_name: tool_name.clone(),
                    effective_input,
                    pre_hook_result,
                    permission_outcome,
                    tool_call_id,
                });
            }

            // ── Pass 2: the waves ──
            // Consecutive tools that may run side by side — reads, and the
            // spawn family — form a wave (`parallel_waves`). A wave runs when
            // Pass 3 reaches its first tool, so it starts only after every
            // ordered tool before it has run: `Edit -> Read` observes the
            // edit, `Bash -> Agent` starts after the shell. Four `Agent` calls
            // in one message ran one after another before, each waiting for
            // the previous subagent to finish although the model had asked
            // for them together, as Claude Code runs its `Task` calls
            // ("병렬로 돌리지 않음", 2026-09-03). Only a batch with no
            // post-tool hooks takes waves; otherwise every tool runs serially
            // in model order so hook order is kept.
            let mut precomputed: HashMap<usize, PrecomputedStreamingToolResult> = HashMap::new();
            let waves = if self.hook_runner.lifecycle_command_count(HookEvent::PostToolUse) == 0
                && self
                    .hook_runner
                    .lifecycle_command_count(HookEvent::PostToolUseFailure)
                    == 0
                && !self.streaming_parallel_batch_has_repetition_risk(&prepared)
            {
                parallel_waves(
                    prepared
                        .iter()
                        .enumerate()
                        .map(|(idx, p)| (idx, p.tool_name.as_str(), &p.permission_outcome)),
                )
            } else {
                Vec::new()
            };
            let mut waves = waves.into_iter().peekable();

            // ── Pass 3: Process results in order ──
            // Precomputed parallel tools have already rendered as they completed;
            // this pass only appends them to the transcript in model order.
            // Non-precomputed tools execute one-by-one. In the live TUI path a
            // `concurrent_dispatch` seam is installed; send every ordinary tool
            // through `spawn_blocking` there so even single Read/Edit/Skill calls
            // yield the turn future instead of freeze-then-bursting the render
            // loop. Sequential `.await`s preserve mutating tool order.
            // Where THIS batch's results begin, so the treadmill can ask what the
            // batch actually accomplished rather than what it merely attempted.
            let batch_results_from = tool_results.len();
            let mut batch_hard_stops = ToolBatchRepetitionHardStops::default();
            for (idx, p) in prepared.iter().enumerate() {
                if waves.peek().is_some_and(|wave| wave[0] == idx) {
                    let wave = waves.next().unwrap_or_default();
                    if let Some(dispatch) = self.concurrent_dispatch.as_ref().map(Arc::clone) {
                        self.run_streaming_wave(
                            &wave,
                            &prepared,
                            &mut precomputed,
                            iterations,
                            &render_tx,
                            &id_gen,
                            message_count_before,
                            dispatch,
                        )
                        .await?;
                    }
                }
                let result_message = if let Some(precomputed_result) = precomputed.remove(&idx) {
                    self.finalize_streaming_tool_result(
                        iterations,
                        p,
                        (precomputed_result.output, precomputed_result.kind),
                        StreamingToolRenderContext {
                            render_tx: &render_tx,
                            id_gen: &id_gen,
                            rollback_message_count: message_count_before,
                        },
                        StreamingToolFinalizeOptions {
                            tool_start: precomputed_result.tool_start,
                            render_result: false,
                            notify_slow: false,
                        },
                        &mut batch_hard_stops,
                    )
                    .await?
                } else {
                    match &p.permission_outcome {
                        PermissionOutcome::Allow => {
                            let synthetic_output = batch_hard_stops.preflight_notice(
                                &p.tool_name,
                                &p.effective_input,
                                || {
                                    self.next_tool_repetition_hard_stop_notice(
                                        &p.tool_name,
                                        &p.effective_input,
                                    )
                                },
                            );
                            if let Some((output, terminates)) = synthetic_output {
                                if terminates {
                                    self.tool_loop_break_requested = true;
                                }
                                self.render_synthetic_streaming_tool_result(
                                    iterations,
                                    p,
                                    &output,
                                    &render_tx,
                                    &id_gen,
                                    message_count_before,
                                )
                                .await?;
                                ConversationMessage::tool_result(
                                    &p.tool_use_id,
                                    &p.tool_name,
                                    merge_hook_context(
                                        p.pre_hook_result.additional_context_messages(),
                                        merge_hook_feedback(
                                            p.pre_hook_result.messages(),
                                            output,
                                            true,
                                        ),
                                    ),
                                    true,
                                )
                            } else {
                            self.record_tool_started(iterations, &p.tool_name);
                            let tool_start = std::time::Instant::now();
                            // Set by the dispatch arm below. A cancelled tool
                            // must not also fire the "this tool is slow"
                            // notification: the user just stopped it, and being
                            // told it was slow a moment later is noise about a
                            // wait they already ended.
                            let mut tool_was_cancelled = false;

                            let (output, kind) = if p.tool_name == "AskUserQuestion" {
                                // AskUserQuestion must be handled async:
                                // `unblock_tool_execute` uses `block_in_place`
                                // which blocks the current task, deadlocking
                                // the TUI select! loop that needs to drain
                                // the prompt and relay the user's answer.
                                match ask_user_question_async(&p.effective_input, &render_tx, &id_gen)
                                    .await
                                {
                                    Ok(answer) => (answer, ToolTextKind::Answer),
                                    Err(e) => (e, ToolTextKind::Failure),
                                }
                            } else {
                                let execution_input = if let Some((delay, input)) =
                                    sleep_tool_execution_input(&p.tool_name, &p.effective_input)
                                {
                                    if !delay.is_zero() {
                                        tokio::time::sleep(delay).await;
                                    }
                                    Cow::Owned(input)
                                } else if let Some(input) = tool_execution_input(
                                    &p.tool_name,
                                    &p.tool_use_id,
                                    &p.effective_input,
                                ) {
                                    Cow::Owned(input)
                                } else {
                                    Cow::Borrowed(p.effective_input.as_str())
                                };
                                if let Some(dispatch) = &self.concurrent_dispatch {
                                    let dispatch = std::sync::Arc::clone(dispatch);
                                    let name = p.tool_name.clone();
                                    let input = execution_input.into_owned();
                                    // Same bracket as the parallel wave: watch
                                    // opened before the tool starts, so only an
                                    // Esc pressed while THIS tool runs settles
                                    // it as cancelled. This is the arm that
                                    // carries Bash and MCP — the wedged calls
                                    // the feature exists for.
                                    let mut watch = self.tool_cancel_signal.watch();
                                    let owner_session = self.session.session_id.clone();
                                    let handle =
                                        tokio::task::spawn_blocking(move || {
                                            match dispatch(&name, &input) {
                                                Ok(output) => (output, ToolTextKind::Answer),
                                                Err(e) => (e.to_string(), e.kind()),
                                            }
                                        });
                                    let (output, kind, cancelled) =
                                        await_cancellable_tool_dispatch(
                                            handle,
                                            &mut watch,
                                            &owner_session,
                                        )
                                        .await
                                        .into_tool_result();
                                    if cancelled {
                                        tool_was_cancelled = true;
                                        self.mark_streaming_tool_cancelled(
                                            iterations,
                                            p,
                                            &render_tx,
                                            &id_gen,
                                            message_count_before,
                                        )
                                        .await?;
                                    }
                                    (output, kind)
                                } else {
                                    unblock_tool_execute(
                                        &mut self.tool_executor,
                                        &p.tool_name,
                                        execution_input.as_ref(),
                                    )
                                }
                            };

                            self.finalize_streaming_tool_result(
                                iterations,
                                p,
                                (output, kind),
                                StreamingToolRenderContext {
                                    render_tx: &render_tx,
                                    id_gen: &id_gen,
                                    rollback_message_count: message_count_before,
                                },
                                StreamingToolFinalizeOptions {
                                    tool_start,
                                    render_result: true,
                                    notify_slow: !tool_was_cancelled,
                                },
                                &mut batch_hard_stops,
                            )
                            .await?
                            }
                        }
                        PermissionOutcome::Deny { reason } => {
                        // Settle the tool card first: the streaming parser
                        // already flipped it to Running, and the TUI only
                        // reconciles a card's status when a ToolResult render
                        // block arrives — without one a denied tool spins
                        // forever under the denial banner.
                        let result_block_id = id_gen.next();
                        if render_tx
                            .send(RenderBlock::ToolResult {
                                id: result_block_id,
                                tool_call_id: p.tool_call_id.clone(),
                                is_error: true,
                                body: format_tool_result_from_raw(&p.tool_name, reason, true),
                            })
                            .await
                            .is_err()
                        {
                            return Err(self.cancel_streaming_turn(
                                iterations,
                                "render channel closed delivering denial result",
                                message_count_before,
                            ));
                        }
                        // A permission-mode mismatch is expected policy
                        // behavior, not a task failure. Show the first one as
                        // a quiet hint and fold later mode denials out of the
                        // transcript; exceptional denials stay warnings.
                        let mode_denial_seen = !self.mode_denial_counts.is_empty();
                        if let Some((level, text)) = permission_denial_notice(
                            &p.tool_name,
                            reason,
                            mode_denial_seen,
                        ) {
                            let deny_block_id = id_gen.next();
                            if render_tx
                                .send(RenderBlock::System {
                                    id: deny_block_id,
                                    level,
                                    text,
                                })
                                .await
                                .is_err()
                            {
                                return Err(self.cancel_streaming_turn(
                                    iterations,
                                    "render channel closed delivering denial",
                                    message_count_before,
                                ));
                            }
                        }
                        let body = self.fold_repeated_mode_denial(
                            &p.tool_name,
                            super::denial_result_body(reason),
                        );
                        ConversationMessage::tool_result(
                            &p.tool_use_id,
                            &p.tool_name,
                            merge_hook_context(
                                p.pre_hook_result.additional_context_messages(),
                                merge_hook_feedback(p.pre_hook_result.messages(), body, true),
                            ),
                            true,
                        )
                    }
                }
                };
                self.session
                    .push_message(result_message.clone())
                    .map_err(|error| StreamingTurnError::runtime(error.to_string()))?;
                self.record_tool_finished(iterations, &result_message, p.effective_input.as_str());
                self.note_progress_tool_result(&result_message);
                tool_results.push(result_message);
            }

            self.arm_tool_repetition_hard_stops();

            // Verification-treadmill circuit breaker (mirrors the sync loop): a
            // batch that plans/validates/spawns (verify-class) but changes no file
            // is a self-verification round; too many in a row without progress stop
            // the turn gracefully. Preserve the well-formed work and end with the
            // marker, like the other budgets.
            let had_verify = tool_uses
                .iter()
                .any(|tool_use| is_verify_class_tool(&tool_use.name));
            let had_mutation =
                batch_had_successful_mutation(&tool_results[batch_results_from..]);
            if self.note_verify_treadmill(had_verify, had_mutation) == TurnContinuity::Stop {
                let error = RuntimeError::new("turn hit the verification treadmill");
                self.clear_empty_retry_reminder(empty_retries);
                self.record_turn_failed(iterations, &error);
                budget_exhausted = Some(BudgetExhausted::VerificationTreadmill);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::VerificationTreadmill,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(StreamingTurnError::runtime)?;
                let _ = render_tx
                    .send(RenderBlock::System {
                        id: id_gen.next(),
                        level: SystemLevel::Warn,
                        text: budget_exhausted_notice(
                            BudgetExhausted::VerificationTreadmill,
                            iterations,
                        ),
                    })
                    .await;
                break 'outer;
            }

            // Mid-turn steering boundary. Fold any user-typed steering into the
            // *last* tool-result message as extra Text blocks rather than a new
            // message: tool-result messages serialize as wire role "user", so a
            // separate user message here would be two consecutive "user" turns
            // (which the API rejects). A user turn may carry tool_result blocks
            // followed by text, so appending keeps one valid turn. `make_mut`
            // mirrors the truncate path already used in this function.
            //
            // Reaching this boundary also resets the gen-abort cap
            // (unconditionally, steers or not): the tool batch this iteration
            // dispatched has already run, so the "stale batch" the
            // mid-generation re-issue exists to prevent is no longer in front
            // of the user, and the next generation is eligible again.
            steering_reissued = false;
            let steers = self.drain_steering();
            if !steers.is_empty() {
                let messages = Arc::make_mut(&mut self.session.messages);
                if let Some(last) = messages.last_mut() {
                    for steer in steers {
                        let _ = render_tx
                            .send(RenderBlock::System {
                                id: id_gen.next(),
                                level: SystemLevel::Info,
                                text: format!("{STEERING_ECHO_PREFIX}{steer}"),
                            })
                            .await;
                        last.blocks.push(ContentBlock::Text {
                            text: steering_message(&steer),
                        });
                    }
                    self.session.mark_transcript_dirty();
                }
            }
            // Mid-turn agent-notification boundary (CC's task-notification
            // contract). Background agents that finished during this tool
            // batch are folded into the same last tool-result message as
            // extra Text blocks, so the main model keeps working through
            // completions instead of ending its turn to receive them. The
            // transcript gets the same collapsible agent-result card the
            // follow-up-turn path renders — mid-turn delivery must not make
            // the result invisible to the user.
            let notifications = self.drain_agent_notifications();
            if !notifications.is_empty() {
                let messages = Arc::make_mut(&mut self.session.messages);
                if let Some(last) = messages.last_mut() {
                    for notification in notifications {
                        let _ = render_tx
                            .send(RenderBlock::AgentResult {
                                id: id_gen.next(),
                                label: notification.label.clone(),
                                status: notification.status,
                                summary: notification.summary.clone(),
                                body: notification.text.clone(),
                            })
                            .await;
                        last.blocks.push(ContentBlock::Text {
                            text: agent_notification_text(&notification),
                        });
                    }
                    self.session.mark_transcript_dirty();
                }
            }
            // Re-anchor the live plan after this tool batch (mirrors the sync
            // turn loop) so the next streamed request keeps the plan in view.
            self.nudge_serial_reads_after_batch();
            self.govern_step_after_batch();
            self.reinject_todo_progress_reminder();
            self.fire_post_batch_lifecycle_hooks();
        }

        if let Some(event) = self.maybe_microcompact_streaming(&render_tx, &id_gen).await {
            microcompact.get_or_insert(event);
        }
        if let Some(event) = self.maybe_auto_compact_streaming(&render_tx, &id_gen).await {
            auto_compaction.get_or_insert(event);
        } else {
            self.maybe_state_distill();
            self.maybe_precompaction_warn_streaming(&render_tx, &id_gen)
                .await;
        }

        let summary = self.build_turn_summary(
            assistant_messages,
            tool_results,
            prompt_cache_events,
            iterations,
            auto_compaction,
            microcompact,
            turn_start_output_tokens,
            budget_exhausted,
        );
        self.record_turn_completed(&summary);

        Ok(summary)
    }
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// One wave: its tools dispatched together through `spawn_blocking`, in
    /// chunks of [`MAX_PARALLEL_SAFE_TOOL_DISPATCHES`], each result rendered
    /// as it completes — a slow first read must not hide later fast results
    /// from the TUI — and kept by index for Pass 3 to append in model order.
    #[allow(clippy::too_many_arguments)]
    async fn run_streaming_wave(
        &mut self,
        wave: &[usize],
        prepared: &[PreparedStreamingTool],
        precomputed: &mut HashMap<usize, PrecomputedStreamingToolResult>,
        iterations: usize,
        render_tx: &mpsc::Sender<RenderBlock>,
        id_gen: &BlockIdGen,
        message_count_before: usize,
        dispatch: ConcurrentDispatchFn,
    ) -> Result<(), StreamingTurnError> {
        for batch in wave.chunks(MAX_PARALLEL_SAFE_TOOL_DISPATCHES) {
            let mut handles = FuturesUnordered::new();
            for &idx in batch {
                let p = &prepared[idx];
                self.record_tool_started(iterations, &p.tool_name);
                let tool_start = std::time::Instant::now();
                let dispatch = Arc::clone(&dispatch);
                let name = p.tool_name.clone();
                // The same execution input the serial pass hands a tool: a
                // spawn carries its own `tool_use` id for manifest attribution.
                let input = tool_execution_input(&p.tool_name, &p.tool_use_id, &p.effective_input)
                    .unwrap_or_else(|| p.effective_input.clone());
                // Opened BEFORE the tool starts, so this dispatch is cancelled
                // only by an Esc that arrives while it is genuinely in flight.
                // Every member of the wave gets its own watch off the same
                // epoch, which is why one keypress settles the whole wave.
                let mut watch = self.tool_cancel_signal.watch();
                let owner_session = self.session.session_id.clone();
                let handle = tokio::task::spawn_blocking(move || match dispatch(&name, &input) {
                    Ok(output) => (output, ToolTextKind::Answer),
                    Err(e) => (e.to_string(), e.kind()),
                });
                handles.push(async move {
                    let outcome =
                        await_cancellable_tool_dispatch(handle, &mut watch, &owner_session).await;
                    (idx, tool_start, outcome)
                });
            }
            while let Some((idx, tool_start, outcome)) = handles.next().await {
                let (output, kind, cancelled) = outcome.into_tool_result();
                if cancelled {
                    self.mark_streaming_tool_cancelled(
                        iterations,
                        &prepared[idx],
                        render_tx,
                        id_gen,
                        message_count_before,
                    )
                    .await?;
                } else {
                    crate::notifications::notify_if_slow(
                        &prepared[idx].tool_name,
                        tool_start,
                        std::time::Duration::from_secs(10),
                    );
                }
                self.render_precomputed_streaming_tool_result(
                    iterations,
                    &prepared[idx],
                    &output,
                    kind,
                    StreamingToolRenderContext {
                        render_tx,
                        id_gen,
                        rollback_message_count: message_count_before,
                    },
                )
                .await?;
                precomputed.insert(
                    idx,
                    PrecomputedStreamingToolResult {
                        output,
                        kind,
                        tool_start,
                    },
                );
            }
        }
        Ok(())
    }

    fn streaming_parallel_batch_has_repetition_risk(&self, prepared: &[PreparedStreamingTool]) -> bool {
        self.parallel_batch_has_repetition_risk(prepared.iter().map(|p| {
            (
                p.tool_name.as_str(),
                p.effective_input.as_str(),
                &p.permission_outcome,
            )
        }))
    }

    async fn render_precomputed_streaming_tool_result(
        &mut self,
        iterations: usize,
        p: &PreparedStreamingTool,
        output: &str,
        kind: ToolTextKind,
        render: StreamingToolRenderContext<'_>,
    ) -> Result<(), StreamingTurnError> {
        let body = if kind.is_error() {
            let output = merge_hook_feedback(
                p.pre_hook_result.messages(),
                output.to_string(),
                false,
            );
            tool_error_body(&p.tool_name, output, kind)
        } else {
            format_tool_result_from_raw(&p.tool_name, output, false)
        };
        let result_block_id = render.id_gen.next();
        if render.render_tx
            .send(RenderBlock::ToolResult {
                id: result_block_id,
                tool_call_id: p.tool_call_id.clone(),
                is_error: kind.is_error(),
                body,
            })
            .await
            .is_err()
        {
            return Err(self.cancel_streaming_turn(
                iterations,
                "render channel closed delivering tool result",
                render.rollback_message_count,
            ));
        }
        Ok(())
    }

    /// Repaint a user-cancelled tool card as `⊘ cancelled` before its synthetic
    /// result lands.
    ///
    /// The result itself carries `is_error: true` (the tool-result wire format
    /// has no third state), and the TUI's reconcile would otherwise settle the
    /// card as a red `×` — indistinguishable from a tool that genuinely blew up.
    /// Re-sending the `ToolCall` block with `ToolCallStatus::Cancelled` merges
    /// in place on `tool_call_id`, so the row reads as "you stopped this", which
    /// is the whole difference the user needs to see.
    async fn mark_streaming_tool_cancelled(
        &mut self,
        iterations: usize,
        p: &PreparedStreamingTool,
        render_tx: &mpsc::Sender<RenderBlock>,
        id_gen: &BlockIdGen,
        rollback_message_count: usize,
    ) -> Result<(), StreamingTurnError> {
        if render_tx
            .send(RenderBlock::ToolCall {
                id: id_gen.next(),
                tool_call_id: p.tool_call_id.clone(),
                name: p.tool_name.clone(),
                summary: tool_summary_line(&p.tool_name, &p.effective_input),
                preview: tool_preview_from(&p.tool_name, &p.effective_input),
                status: ToolCallStatus::Cancelled,
            })
            .await
            .is_err()
        {
            return Err(self.cancel_streaming_turn(
                iterations,
                "render channel closed marking a cancelled tool",
                rollback_message_count,
            ));
        }
        // The cancel is committed here: the card now reads "you stopped this"
        // and the caller settles `CANCELLED_TOOL_RESULT` for this same tool
        // next. Both dispatch arms — the parallel-safe wave and the single
        // concurrent dispatch — funnel through this one helper, so a cancelled
        // tool is counted exactly once and neither arm can drift from the other.
        //
        // The tool NAME is deliberately not carried: attestation reasons are
        // `&'static str` by construction so the table's cardinality stays
        // bounded by the code's, and per-call detail is a stated non-goal of
        // the ledger (see the `harness_attest` module doc).
        telemetry::attest_fired(telemetry::HarnessFeature::ToolCancelSettled);
        Ok(())
    }

    async fn render_synthetic_streaming_tool_result(
        &mut self,
        iterations: usize,
        p: &PreparedStreamingTool,
        output: &str,
        render_tx: &mpsc::Sender<RenderBlock>,
        id_gen: &BlockIdGen,
        rollback_message_count: usize,
    ) -> Result<(), StreamingTurnError> {
        let result_block_id = id_gen.next();
        if render_tx
            .send(RenderBlock::ToolResult {
                id: result_block_id,
                tool_call_id: p.tool_call_id.clone(),
                is_error: true,
                body: format_tool_result_from_raw(&p.tool_name, output, true),
            })
            .await
            .is_err()
        {
            return Err(self.cancel_streaming_turn(
                iterations,
                "render channel closed delivering synthetic tool result",
                rollback_message_count,
            ));
        }
        Ok(())
    }

    async fn finalize_streaming_tool_result(
        &mut self,
        iterations: usize,
        p: &PreparedStreamingTool,
        result: (String, ToolTextKind),
        render: StreamingToolRenderContext<'_>,
        options: StreamingToolFinalizeOptions,
        batch_hard_stops: &mut ToolBatchRepetitionHardStops,
    ) -> Result<ConversationMessage, StreamingTurnError> {
        let (mut output, kind) = result;
        let mut is_error = kind.is_error();
        if options.notify_slow {
            crate::notifications::notify_if_slow(
                &p.tool_name,
                options.tool_start,
                std::time::Duration::from_secs(10),
            );
        }
        // Successful structured renders must use the pristine tool output,
        // not hook-merged text, or edit/read cards degrade into generic
        // strings. Preserve that pristine string only when pre-hook feedback
        // will overwrite `output` before we know the final render status.
        // If there is no pre-hook feedback, `output` remains the pristine
        // string until after the success render body is built, so no clone is
        // needed even when post-hook feedback will later be appended for the
        // model-facing transcript.
        let pure_output_for_render = (!is_error && !p.pre_hook_result.messages().is_empty())
            .then(|| output.clone());
        output = merge_hook_feedback(p.pre_hook_result.messages(), output, false);

        // Async seam (same off-task offload as the pre-hook): the blocking
        // post-hook subprocess runs on a `spawn_blocking` worker so this
        // streaming task stays responsive while the hook runs.
        let post_hook_result = if is_error {
            self.run_post_tool_use_failure_hook_async(&p.tool_name, &p.effective_input, &output)
                .await
        } else {
            self.run_post_tool_use_hook_async(&p.tool_name, &p.effective_input, &output, false)
                .await
        };
        let post_hook_is_error = post_hook_result.is_denied()
            || post_hook_result.is_failed()
            || post_hook_result.is_cancelled();
        if post_hook_is_error {
            is_error = true;
        }

        let body = if is_error {
            output = merge_hook_feedback(post_hook_result.messages(), output, post_hook_is_error);
            tool_error_body(&p.tool_name, output.clone(), kind)
        } else {
            let render_output = pure_output_for_render.as_deref().unwrap_or(&output);
            let body = format_tool_result_from_raw(&p.tool_name, render_output, false);
            output = merge_hook_feedback(post_hook_result.messages(), output, false);
            body
        };
        // The rendered body is settled above; what a hook contributed as
        // `additionalContext` joins the MODEL-facing result only. The error arm
        // is why this sits here rather than beside the merges: there `output`
        // IS the rendered body, and folding context in earlier printed a hook's
        // prompt material into the tool-error cell.
        output = merge_hook_context(p.pre_hook_result.additional_context_messages(), output);
        output = merge_hook_context(post_hook_result.additional_context_messages(), output);
        if options.render_result {
            let result_block_id = render.id_gen.next();
            if render
                .render_tx
                .send(RenderBlock::ToolResult {
                    id: result_block_id,
                    tool_call_id: p.tool_call_id.clone(),
                    is_error,
                    body,
                })
                .await
                .is_err()
            {
                return Err(self.cancel_streaming_turn(
                    iterations,
                    "render channel closed delivering tool result",
                    render.rollback_message_count,
                ));
            }
        }
        // Enforcer-layer denials surface as tool errors here; fold same-class
        // repeats on the model-facing result like the policy-layer deny arm.
        if is_error {
            output = self.fold_repeated_mode_denial(&p.tool_name, output);
        }
        // Append (after rendering, so the user-facing body stays clean) only to
        // the model-facing result, nudging the agent out of a tight
        // identical-call loop.
        self.append_tool_repetition_notice(
            &mut output,
            &p.tool_name,
            &p.effective_input,
            is_error,
            batch_hard_stops,
        );

        // Drain images the tool staged. The real live dispatcher shares the
        // image sink through cloned contexts, so this remains correct whether
        // the serial tool executed directly or through the spawn_blocking
        // dispatch seam.
        let images = self.tool_executor.take_pending_images();
        Ok(tool_result_message(
            &p.tool_use_id,
            &p.tool_name,
            output,
            is_error,
            images,
        ))
    }
}

#[cfg(test)]
mod network_wait_tests {
    /// The probe reads the link, not the provider: a port that refuses is a
    /// network that carried the packet, and a black hole is not.
    #[tokio::test]
    async fn the_network_probe_tells_a_listener_from_a_black_hole() {
        // A port something listens on is a link that works.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        assert!(super::network_reaches_port("127.0.0.1", port).await, "a listener is reachable");
        // A host that names nothing is answered optimistically: the re-run is
        // the probe then.
        assert!(super::network_reaches("").await);
        // A name that resolves to nothing is no link at all.
        assert!(!super::network_reaches("nonexistent.invalid").await);
        // RFC 5737 TEST-NET-1 is unroutable: the connect times out or the
        // local stack rejects it, and either is "no link".
        let started = std::time::Instant::now();
        assert!(!super::network_reaches("192.0.2.1").await, "a black hole is not reachable");
        assert!(started.elapsed() <= super::NETWORK_PROBE_TIMEOUT + std::time::Duration::from_secs(2));
        drop(listener);
    }

    /// The wait notice names the host and the clock, and the restored line
    /// says the interrupted answer starts over — nothing of it ran.
    #[test]
    fn the_wait_notice_names_host_and_clock() {
        let text = super::network_wait_warn(
            "chatgpt.com",
            std::time::Duration::from_secs(4),
            std::time::Duration::from_secs(6),
            2,
        );
        assert_eq!(
            text,
            "network unreachable (chatgpt.com); waiting for the connection — next check in 4s, waited 6s (attempt 2)"
        );
        assert!(super::network_wait_warn("", std::time::Duration::ZERO, std::time::Duration::ZERO, 1)
            .contains("the provider"));
        assert!(super::NETWORK_RESTORED_INFO.contains("starts over"));
    }
}
