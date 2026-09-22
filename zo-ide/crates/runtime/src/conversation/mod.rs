use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use core_types::MemoryRetriever;
use serde_json::json;
use telemetry::SessionTracer;

use crate::model_router::{RouteTaskComplexity, RouteTaskIntent, RouteTaskRisk};

mod api;
mod budget_wrap_up;
mod cache_breaks;
mod timing_ledger;
mod compaction;
mod config;
mod deep_gate;
mod error;
mod fallback;
mod helpers;
mod reminders;
mod repetition;
mod reviewed_edit;
mod step_effort;
mod streaming;
mod streaming_turn;
mod team_inbox;
mod tool;
mod tool_call_salvage;
mod turn_end;
mod turn_end_gate;
mod turn_support;
mod verified_state;
mod verify_treadmill;

pub use api::{
    flush_pending_tool_events, prompt_cache_record_to_event, record_non_anthropic_prompt_cache_usage, push_output_block, redacted_thinking_data_to_string, response_to_events, ApiClient, ApiRequest,
    AssistantEvent, AsyncApiClient, PromptCacheEvent, ProviderStateBlob,
    DEFAULT_STREAMING_CHANNEL_CAPACITY,
};
use compaction::{auto_compaction_threshold_from_env_or_policy, ContextPolicy};
pub use compaction::{
    auto_compaction_tail_budget, auto_compaction_threshold_for_model, auto_compaction_threshold_from_env, MICROCOMPACT_MIN_OUTPUT_BYTES,
    count_progress_tool_results, final_assistant_text, AutoCompactionEvent, BudgetExhausted,
    TurnSummary,
};
pub use config::{
    declare_attendance, declared_attendance, env_deadline_extension, env_turn_budgets, Attendance,
    DEFAULT_TURN_DEADLINE_SECS,
    DEFAULT_TURN_INPUT_TOKEN_BUDGET, DEFAULT_TURN_OUTPUT_TOKEN_BUDGET,
};
#[allow(unused_imports)]
pub use compaction::auto_compaction_threshold_from_env_or_model;
pub use deep_gate::{
    bash_result_exited_zero, detect_check_command, read_only_bash_allow_rules,
    take_verifier_calibration_events,
    DeepGateConfig, DeepMode, DeepOutcome, ExecContract, VerifierCalibrationEvent,
    AUTO_RETRY_MARKER, DEEP_EXEC_MARKER, DEEP_PLAN_MARKER, DEEP_VERIFY_MARKER,
};
// `parse_auto_compaction_threshold` is only touched from `#[cfg(test)]`,
// re-exported here so the test mod's `super::` paths keep working.
#[cfg(test)]
use compaction::parse_auto_compaction_threshold;
pub use error::{RuntimeError, StreamingTurnError, ToolError, ToolTextKind};
pub use step_effort::{
    decide as decide_step_effort, shift as shift_step_effort, EffortStep, RungMove, StepAsk,
    StepAskContext, StepBatch, StepDecision, StepEffortConfig, StepEffortObserver, StepEffortSeat,
    StepEvent, StepJudgment, StepJudgmentRow, StepLabel, StepMove, StepReason, StepRow,
    StepSignals, LABEL_ROW_KIND, NO_IN_TURN_ROAD, ROUTINE_STEPS_FOR_LIGHTER, STEP_JUDGMENT_EVERY,
    STEP_ROW_KIND, STRONG_STEPS_FOR_HEAVIER,
};
pub use reminders::{
    build_design_guidance_reminder, DESIGN_GUIDANCE_REMINDER_PREFIX, PRELUDE_FANNED_OUT_REMINDER,
    ROUTE_HINT_REMINDER_PREFIX,
};
pub use tool::{ConcurrentDispatchFn, LongRunningPredicate, StaticToolExecutor, ToolExecutor};
use tool::rides_a_wave;

use helpers::{
    ask_user_question_async, build_assistant_message, estimate_system_prompt_tokens,
    format_hook_message, merge_hook_context, merge_hook_feedback,
    normalize_empty_assistant_stream, trace_attrs,
    AssistantTurn,
};
use streaming::{
    build_async_permission_request, tool_preview_from, tool_summary_line, CapturePrompter,
};
use tool::{
    is_concurrency_safe, is_long_running, sleep_tool_execution_input, tool_execution_input,
    unblock_tool_execute,
};
use repetition::{ReadFileRange, ToolBatchRepetitionHardStops};
// Turn-stop policy (cost breaker vs behavior heuristic). Shared with the
// benchmark harness through `decision-core`, so the live loops cannot drift into
// a second copy of the rule. Child modules reach these through `super::`.
use decision_core::{decide_turn_continuity, TurnContinuity, TurnStopSignal};
// Verification-treadmill guard: the loop-boundary predicate the sync + streaming
// turn loops call to classify a tool batch. `note_verify_treadmill` (the counter
// + advisory) is an inherent-impl method, reachable without an import.
use verify_treadmill::{batch_had_successful_mutation, is_verify_class_tool};
pub(crate) use verified_state::receipt_in;
// Refusal/quota-fallback items the sync + streaming turn loops still reference.
use fallback::{
    is_refusal_stop_reason, overload_demotion_warn, quota_fallback_swap_warn,
    quota_wait_hold_warn,
    refusal_surfaced_message, QuotaEscape, RefusalDecision,
    REFUSAL_DRY_PREARM_WARN, REFUSAL_FALLBACK_WARN, REFUSAL_SURFACED_NOTICE,
};
// Turn-completion items the turn loops + `deep_gate` still reference.
use turn_end::{
    budget_exhausted_notice, build_turn_end_hook_context, changed_files_snapshot,
    changed_files_snapshot_async,
};
// Reminder items the staying prompt-submit/hook code + `compaction` still reference.
use reminders::{
    build_user_prompt_hook_context_reminder, escape_low_trust_reminder_body,
    STATE_DISTILL_REMINDER_PREFIX, TODO_PROGRESS_REMINDER_PREFIX,
    USER_PROMPT_HOOK_CONTEXT_REMINDER_PREFIX,
};
// Reminder seams the `#[cfg(test)] mod tests` reaches through `super::`.
#[cfg(test)]
use reminders::{
    todo_progress_reminder_for, GOAL_CLARIFY_REMINDER_PREFIX, RECALL_HINT_REMINDER_PREFIX,
    USER_PROMPT_HOOK_CONTEXT_MAX_CHARS, USER_PROMPT_HOOK_CONTEXT_TRUNCATED_MARKER,
};
// Spec-literal-gate seams the `#[cfg(test)] mod tests` reaches through `super::`.
#[cfg(test)]
use turn_end::{original_has_candidate_spec_literals, GATE_CHANGED_FILES_CALLS};
// Repetition-guard items the `#[cfg(test)] mod tests` reaches through `super::`.
#[cfg(test)]
use repetition::{
    fingerprint_tool_call, per_turn_tool_repetition_nonterminating_notice, record_tool_fingerprint,
    skipped_after_repetition_stop_notice, ToolRepetition, TOOL_REPETITION_CROSS_TURN_ADVISE,
    TOOL_REPETITION_CROSS_TURN_HARD_STOP, TOOL_REPETITION_HARD_STOP, TOOL_REPETITION_THRESHOLD,
};

use crate::compact::{estimate_session_tokens, CompactionConfig, CompactionResult};
use crate::config::RuntimeFeatureConfig;
use crate::hooks::{HookAbortSignal, HookEvent, HookProgressReporter, HookRunResult, HookRunner};
use crate::memory::render_recalled_memory_section;
use crate::message_stream::anthropic::tools::format_tool_result_from_raw;
use crate::message_stream::types::{
    BlockIdGen, RenderBlock, SystemLevel, ToolCallId, ToolCallStatus,
};
use crate::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionPrompter as AsyncPermissionPrompter,
};
use crate::permissions::{
    PermissionContext, PermissionMode, PermissionOutcome, PermissionPolicy,
    PermissionPromptDecision, PermissionPrompter, TemporaryAllowGrant,
};
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};
use crate::team_inbox_digest::TeamInboxDeliveryBatch;
use crate::usage::{TokenUsage, UsageTracker};

/// Fraction of the model context window used as the default auto-compaction
/// threshold (85 %). When no explicit env-var override is set, the runtime
/// computes `context_window * 85 / 100` so larger-context models enjoy a
/// proportionally higher compaction boundary.
///
/// Set to 85% to match Claude Code's behaviour: keep the live context filling
/// up and only compact once the window is nearly exhausted, instead of
/// compacting early and often. This trades a little late-window recall (the
/// "context rot" long-context studies — `NoLiMa` (`arXiv:2502.05167`) and Chroma's
/// 2025 "Context Rot") for far fewer summarisation passes and a longer
/// verbatim history, which is what users expect from a Claude-style CLI. The
/// `CLAUDE_CODE_AUTO_COMPACT_INPUT_TOKENS` env var still overrides this with an
/// absolute token budget.
const AUTO_COMPACTION_CONTEXT_WINDOW_PERCENT: u64 = 85;

/// Legacy hard-coded fallback used when no model is known **and** the
/// env-var override is absent.  Matches the previous static default.

#[derive(Debug, Clone)]
struct PendingToolUse {
    id: String,
    name: String,
    input: String,
}

fn collect_pending_tool_uses(message: &ConversationMessage) -> Vec<PendingToolUse> {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(PendingToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            _ => None,
        })
        .collect()
}

const FALLBACK_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD: u32 = 100_000;

/// Default `max_output_tokens` assumed for the overflow guard when the
/// true value is not available.  8 192 matches the Anthropic default for
/// most models.
const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 8_192;

const MAX_SLEEP_TOOL_DURATION_MS: u64 = 5_000;
const MAX_PARALLEL_SAFE_TOOL_DISPATCHES: usize = 8;

/// Number of project memory entries recalled into a single model request.
const DEFAULT_MEMORY_RECALL_LIMIT: usize = 5;

// The render-time entry clamp must cover the recall limit, or well-behaved
// retrievers would have legit hits dropped — and the compaction preflight
// reserve ([`crate::memory::recall::recall_section_reserve_tokens`]) is sized for
// the clamp, so the clamp must be the true upper bound on rendered entries.
const _: () = assert!(DEFAULT_MEMORY_RECALL_LIMIT <= crate::memory::recall::MAX_RECALLED_ENTRIES);

/// How many extra times to re-request when the assistant stream finishes
/// cleanly but carries no content (a transient empty completion). Empties are
/// usually one-off, so a couple of retries recover the turn instead of
/// discarding it; after this many we synthesize a visible fallback assistant
/// message rather than leaving the user turn with no recorded assistant output.
/// See [`AssistantTurn::Empty`].
const MAX_EMPTY_STREAM_RETRIES: usize = 2;
const EMPTY_STREAM_RETRY_REMINDER_PREFIX: &str = "[zo:empty-response-retry]";
const EMPTY_STREAM_RETRY_REMINDER: &str = "[zo:empty-response-retry] <system-reminder>The previous assistant response to this same request ended with no text or tool call. Retry now, and do not end this turn empty. Produce at least one visible text response or a tool call.</system-reminder>";
/// Empty-retry reminder for the *truncation* sub-case: the turn ended empty
/// because the provider cut it off at the output-token limit while the model
/// was still reasoning, before it emitted any answer or tool call. Re-requesting
/// verbatim re-spends the same window the same way, so the reminder tells the
/// model to spend far less on reasoning and act directly. Shares the empty-retry
/// prefix so the existing cleanup clears it. See [`is_truncation_stop_reason`].
const EMPTY_STREAM_TRUNCATION_RETRY_REMINDER: &str = "[zo:empty-response-retry] <system-reminder>Your previous response was cut off at the output-token limit while reasoning, before producing any text or tool call — your reasoning needs more room than the budget allows. Retry with MINIMAL reasoning: go straight to the answer or the next tool call now.</system-reminder>";
const EMPTY_STREAM_CONTINUATION_REMINDER_PREFIX: &str = "[zo:empty-response-continuation]";
const EMPTY_STREAM_CONTINUATION_REMINDER: &str = "[zo:empty-response-continuation] <system-reminder>The previous turn ended because the upstream model returned no assistant text or tool call after retries. The session state is intact: the latest user request, assistant tool calls, and tool results remain valid context. Provide the missing final answer or the next necessary tool call; do not repeat the fallback notice.</system-reminder>";
const EMPTY_STREAM_EXHAUSTED_FALLBACK_TEXT: &str = "The model returned no assistant content after retries. The session and any tool work from this turn were preserved for the next turn.";

/// How many times a single turn may be auto-continued after the provider cut
/// the response off at the output-token limit (`stop_reason = "max_tokens"` /
/// `"length"`). A truncated turn is *incomplete*, not finished: the model often
/// spent the whole window reasoning and stopped before emitting the tool call
/// it had already decided on, so treating that as the turn's end silently drops
/// the deliverable (observed on greenfield headless builds at high effort). We
/// preserve the partial output, nudge the model to continue concisely, and
/// re-request. Bounded so a model that keeps over-spending the window cannot
/// loop forever — each continuation has the prior partial output in context, so
/// genuine progress converges within one or two passes.
const MAX_TRUNCATION_CONTINUATIONS: usize = 3;
const TRUNCATION_CONTINUATION_REMINDER: &str = "[zo:truncation-continuation] <system-reminder>Your previous response was cut off at the output-token limit before you finished — the task is NOT complete. The partial response is already in context; avoid re-planning or repeating earlier reasoning. Take the next concrete action now (call the tool you were about to call / write the file). Keep reasoning minimal and act.</system-reminder>";

/// Whether a provider stop reason means the turn was truncated at the
/// output-token limit (Anthropic `"max_tokens"`, OpenAI `"length"`) rather than
/// ending naturally (`"end_turn"`, `"tool_use"`, `"stop"`). A truncated turn is
/// incomplete and should be continued, not treated as completion.
fn is_truncation_stop_reason(reason: &str) -> bool {
    matches!(reason, "max_tokens" | "length")
}




/// Thinking-budget floor the deep-gate escalates to on a stalled retry. Equal to
/// the `Xhigh` preset (the `effort_picker` SSOT): the bench shows genuinely hard
/// tasks that `High` cannot solve pass at `Xhigh`, while `Max` only over-thinks
/// without converging — so escalation never *raises* effort past `Xhigh`.
/// Applied as a floor (`max(configured, this)`): it only raises, so a task the
/// user already configured at `Max` keeps `Max` on a retry — escalation just
/// never introduces `Max` on its own.
const ESCALATION_EFFORT_BUDGET: u32 = 16_000;

/// Default cap on consecutive `TurnEnd`-hook continuations (the Stop-loop). A
/// `TurnEnd` hook returning a `followupMessage` re-injects it as the next user
/// message and runs another turn; this bounds "keep going until done" loops so
/// a misbehaving hook can never spin forever. Without a `followupMessage` the
/// loop runs exactly once, so this is inert unless a Stop hook opts in.
const DEFAULT_MAX_STOP_LOOPS: usize = 10;

/// How many times per turn a *behavior heuristic* (the verification treadmill,
/// the tool-call repetition guards) may be demoted from "end the turn" to "warn
/// and keep going" on the strength of fresh objective progress. One: a
/// demonstrably productive turn earns a single reprieve, and the guard's
/// original stop still lands if the same pattern repeats. Turn-scoped and
/// SHARED across every heuristic, so two different guards cannot each grant
/// their own extension. Cost breakers are never eligible — see
/// [`decision_core::turn_continuity`].
const HEURISTIC_STOP_MAX_NUDGES: u32 = 1;

/// The turn-loop iteration cap. Persistent interactive turns are unbounded by
/// default because a human can interrupt them and the repetition guard already
/// stops identical tool-call thrash. Unattended callers install their own finite
/// caps; `ZO_MAX_ITERATIONS` remains an explicit operator override.
fn default_max_iterations() -> usize {
    std::env::var("ZO_MAX_ITERATIONS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(usize::MAX)
}

/// System message injected into the session after automatic compaction so
/// the model is aware that earlier context has been summarized. Carries the
/// same LAVA recovery affordance as [`COMPACTION_RESUME_REMINDER`]: the
/// compaction that injects this reminder sealed the evicted originals to the
/// vault, so summarized detail is recoverable via `session_recall` — without
/// naming that here, the model treats it as lost exactly on the long sessions
/// where recovery matters most.
const POST_COMPACTION_SYSTEM_REMINDER: &str = "[system: Prior conversation context was automatically compacted to manage context window. The conversation continues with a summary of earlier messages. The original pre-compaction messages are preserved and recoverable: call session_recall with this session id (or \"latest\") and a query to pull back exact detail the summary omits.]";

/// Re-injected when a runtime is built from a session that was already compacted
/// — i.e. on a cold `--resume`, where the in-memory [`POST_COMPACTION_SYSTEM_REMINDER`]
/// (and its todo/edited-files companions) added live by `finish_auto_compaction`
/// did not survive to disk. Beyond restoring compaction-awareness, it surfaces
/// the LAVA recovery affordance: the pre-compaction originals are preserved in
/// the vault and reachable via `session_recall`, so the model can pull back an
/// exact detail the summary omitted instead of treating it as lost.
const COMPACTION_RESUME_REMINDER: &str = "[system: This session was compacted earlier — older messages were summarized to fit the context window. The original pre-compaction messages are preserved and recoverable: call session_recall with this session id (or \"latest\") and a query to pull back exact detail the summary omits.]";

fn format_auto_compaction_start_notice(message_count: usize) -> String {
    // Claude Code-style notice: a single "Compacting conversation…" line shown
    // while the summary request is in flight. The CLI upgrades this into a live
    // status indicator; the wording is kept stable so both surfaces match.
    format!("Compacting conversation… (summarizing {message_count} messages)")
}

fn format_auto_compaction_done_notice(
    removed_message_count: usize,
    tokens_before: usize,
    tokens_after: usize,
) -> String {
    format!(
        "Compacted conversation · {removed_message_count} messages summarized · {} → {} tokens",
        format_kilo_tokens(tokens_before),
        format_kilo_tokens(tokens_after)
    )
}

/// `254_100` → `"254.1k"`, sub-thousand values stay raw. Matches the HUD's
/// ctx-figure style so the done notice reads on the same scale.
fn format_kilo_tokens(tokens: usize) -> String {
    if tokens >= 1_000 {
        let tenths = tokens / 100;
        format!("{}.{}k", tenths / 10, tenths % 10)
    } else {
        tokens.to_string()
    }
}

/// Pre-compaction early-warning line (Claude Code "Context low… auto-compact
/// soon" parity): a single heads-up shown when the session first climbs into the
/// band just below the full auto-compaction ceiling, so the user can `/compact`
/// at a clean moment instead of being surprised by an auto round mid-thought.
///
/// The transcript persists after micro-compaction changes live occupancy, so it
/// names only the stable threshold instead of leaving a stale percentage that
/// looks like a fourth live `ctx` reading.
fn format_precompaction_warning(compact_percent: u64) -> String {
    format!(
        "Context nearing auto-compaction — threshold {compact_percent}% of window (use /compact to choose the moment)"
    )
}

fn format_microcompact_notice(event: crate::MicrocompactEvent) -> String {
    let saved = event.estimated_tokens_saved;
    let amount = if saved >= 1_000 {
        format!("~{}k tokens", saved / 1_000)
    } else {
        format!("~{saved} tokens")
    };
    let images = if event.cleared_images > 0 {
        format!(" + {} old image(s)", event.cleared_images)
    } else {
        String::new()
    };
    let reminders = if event.cleared_superseded_reminders > 0 {
        format!(
            " + {} superseded reminder(s)",
            event.cleared_superseded_reminders
        )
    } else {
        String::new()
    };
    format!(
        "Context trim · cleared {} old tool result(s){images}{reminders} ({amount} freed)",
        event.cleared_results
    )
}

fn empty_stream_exhausted_message() -> ConversationMessage {
    ConversationMessage::assistant(vec![ContentBlock::Text {
        text: EMPTY_STREAM_EXHAUSTED_FALLBACK_TEXT.to_string(),
    }])
}


/// Preamble prepended to every mid-turn steering message folded into the live
/// conversation. Phrased as an authoritative course correction (D): the user
/// typed this *after* the turn started, so it takes priority over and may
/// override the original request. Kept as one constant so the tool-result and
/// the text-only-turn steering paths inject identical wording.
///
/// The absorption clause is the orchestrator policy: a steer that adds a NEW,
/// separable requirement must not stall the work already in flight — it is
/// delegated to a background agent and the current lane keeps running, which
/// is what makes typing mid-turn cheap for the user instead of a context
/// switch that serializes everything behind it.
const STEERING_PREAMBLE: &str = "[User steering — the user sent this mid-turn to correct course. Treat it as a higher-priority instruction that supersedes any conflicting earlier guidance, and adjust your plan and current work accordingly before continuing. If it introduces a NEW requirement that is separable from what you are doing right now, do not serialize it: launch a background `Agent` for it immediately and keep working on your current task — you will be notified when that agent completes. Fold it into your current work instead only when it modifies the very thing you are editing.]";

/// Transcript echo prefix stamped when a mid-turn steering message is folded
/// into the live turn. The TUI matches on this exact prefix to clear that
/// message's "queued" badge — keep every emitter on this constant.
pub const STEERING_ECHO_PREFIX: &str = "\u{2937} steering: ";

/// The frame a peer's message wears when `SendMessage` steers it into a running
/// session: `[message via SendMessage from <name>] <body>`, or without
/// ` from <name>` when the sender has no handle. The receiving model tells it
/// apart from its own task text, and the receiving TUI draws it as a message
/// from someone else rather than as the person's own words ("메시지 표시도
/// 깔끔하게", 2026-09-06). One writer ([`peer_message_frame`]) and one reader
/// ([`parse_peer_message`]), here, so the two cannot drift apart.
pub const PEER_MESSAGE_FRAME_OPEN: &str = "[message via SendMessage";
const PEER_MESSAGE_FRAME_FROM: &str = " from ";
const PEER_MESSAGE_FRAME_CLOSE: &str = "] ";

/// Frame `body` as a peer's message, naming the sender when it has a handle.
#[must_use]
pub fn peer_message_frame(from: Option<&str>, body: &str) -> String {
    match from.map(str::trim).filter(|from| !from.is_empty()) {
        Some(from) => format!(
            "{PEER_MESSAGE_FRAME_OPEN}{PEER_MESSAGE_FRAME_FROM}{from}{PEER_MESSAGE_FRAME_CLOSE}{body}"
        ),
        None => format!("{PEER_MESSAGE_FRAME_OPEN}{PEER_MESSAGE_FRAME_CLOSE}{body}"),
    }
}

/// A framed peer message, read back: who sent it (when the frame says) and
/// what they said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerMessage<'a> {
    pub from: Option<&'a str>,
    pub body: &'a str,
}

/// Read a [`peer_message_frame`] back; `None` for any other text.
#[must_use]
pub fn parse_peer_message(text: &str) -> Option<PeerMessage<'_>> {
    let rest = text.strip_prefix(PEER_MESSAGE_FRAME_OPEN)?;
    if let Some(body) = rest.strip_prefix(PEER_MESSAGE_FRAME_CLOSE) {
        return Some(PeerMessage { from: None, body });
    }
    let named = rest.strip_prefix(PEER_MESSAGE_FRAME_FROM)?;
    let close = named.find(PEER_MESSAGE_FRAME_CLOSE)?;
    let from = &named[..close];
    let body = &named[close + PEER_MESSAGE_FRAME_CLOSE.len()..];
    Some(PeerMessage {
        from: (!from.is_empty()).then_some(from),
        body,
    })
}

#[cfg(test)]
mod peer_message_frame_tests {
    use super::{parse_peer_message, peer_message_frame, PeerMessage};

    /// The frame round-trips with and without a sender, and ordinary text —
    /// including a body that merely mentions the tool — is not a peer message.
    #[test]
    fn a_peer_message_frame_reads_back_its_sender_and_body() {
        let framed = peer_message_frame(Some("reviewer"), "look at src/lib.rs\nthen report");
        assert_eq!(framed, "[message via SendMessage from reviewer] look at src/lib.rs\nthen report");
        assert_eq!(
            parse_peer_message(&framed),
            Some(PeerMessage {
                from: Some("reviewer"),
                body: "look at src/lib.rs\nthen report",
            })
        );
        let anonymous = peer_message_frame(Some("  "), "hello");
        assert_eq!(anonymous, "[message via SendMessage] hello");
        assert_eq!(
            parse_peer_message(&anonymous),
            Some(PeerMessage {
                from: None,
                body: "hello",
            })
        );
        assert_eq!(parse_peer_message("please use SendMessage to reply"), None);
        assert_eq!(parse_peer_message("[message via SendMessage"), None);
    }
}

/// Render the steering body the model sees, with the authoritative preamble.
fn steering_message(steer: &str) -> String {
    format!("{STEERING_PREAMBLE}\n{steer}")
}

/// Preamble prepended to a background-agent result folded into the live turn
/// at a tool-result boundary (CC's `<task-notification>` contract). Distinct
/// from [`STEERING_PREAMBLE`]: this is a host notification, not a user
/// instruction, so it must not read as a course correction — the model folds
/// the result into its ongoing work instead of pivoting to it.
const AGENT_NOTIFICATION_PREAMBLE: &str = "[Task notification — a background agent you launched finished while this turn was still running. Its result follows. This is a host notification, not a user message: use the result now where it affects your current work, and account for it before ending the turn.]";

/// Render the mid-turn agent-notification body the model sees.
///
/// Only a COMPLETION gets the task-notification preamble. A mid-run
/// agent→main message arrives fully framed from the host (teammate wrapper +
/// anti-spoof paragraph + reply trailer) precisely so the mid-turn fold and
/// the idle follow-up turn deliver byte-identical text; prepending the
/// "an agent you launched finished" preamble to it would be a lie.
fn agent_notification_text(notification: &AgentNotification) -> String {
    match notification.kind {
        AgentNotificationKind::Completion => {
            format!("{AGENT_NOTIFICATION_PREAMBLE}\n\n{}", notification.text)
        }
        AgentNotificationKind::Message { .. } => notification.text.clone(),
    }
}

/// Compact, single-line denial banner for the transcript. The permission
/// layer's full reason (audit trail + remediation commands) still reaches the
/// model verbatim in the `tool_result`; the on-screen banner keeps only the
/// first sentence so an expected-mode denial (e.g. `bash` in read-only) reads
/// as one calm line instead of a multi-line warning wall. `/permissions`
/// remains the discoverable inspection path and is appended as a short hint.
fn denial_banner(tool_name: &str, reason: &str) -> String {
    let first = reason
        .split("Permission audit:")
        .next()
        .unwrap_or(reason)
        .trim()
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    let head = if first.is_empty() { reason } else { first };
    format!("denied '{tool_name}': {head} \u{00b7} /permissions")
}

/// Classify a denied call for the transcript without changing the full,
/// model-visible tool result. A permission-mode mismatch is an expected policy
/// skip: show one quiet hint per turn, then suppress repeats. Hook blocks,
/// explicit user denials, and other exceptional reasons remain warnings.
fn permission_denial_notice(
    tool_name: &str,
    reason: &str,
    mode_denial_seen: bool,
) -> Option<(SystemLevel, String)> {
    let mode_notice = (|| {
        let audit = denial_audit_class(reason)?;
        let modes = audit.strip_prefix("Permission audit: active mode is ")?;
        let (active, required) = modes.split_once("; required mode is ")?;
        let active = active.trim();
        let required = required.trim().trim_end_matches('.');
        let prelude = reason.split("Permission audit:").next()?.trim();
        if active.is_empty()
            || required.is_empty()
            || !prelude.contains(&format!("current mode is {active}"))
            || !prelude.contains(&format!("requires {required} permission"))
        {
            return None;
        }
        Some(format!(
            "{active} mode skipped '{tool_name}' (needs {required}) \u{00b7} /permissions"
        ))
    })();

    match mode_notice {
        Some(_) if mode_denial_seen => None,
        Some(text) => Some((SystemLevel::Info, text)),
        None => Some((SystemLevel::Warn, denial_banner(tool_name, reason))),
    }
}

/// Model-visible body for a denied tool call (never the on-screen banner).
/// A denial is the user's decision, not a transient failure — without this
/// framing, weaker models re-issue the identical call and burn the turn.
fn denial_result_body(reason: &str) -> String {
    format!(
        "{reason}\nThis call was declined — adjust your approach (a different tool, a narrower \
         input, or ask what is permitted); do not retry the same call verbatim."
    )
}

/// The "Permission audit: …" sentence identifies a denial's (active mode,
/// required mode) class independent of the specific command/input, so
/// repeated denials of the same class can be folded per turn.
fn denial_audit_class(text: &str) -> Option<&str> {
    let start = text.find("Permission audit:")?;
    let rest = &text[start..];
    Some(rest.find(". ").map_or(rest, |end| &rest[..end]))
}


fn lifecycle_hook_outcome(result: &HookRunResult) -> &'static str {
    if result.is_cancelled() {
        "cancelled"
    } else if result.is_failed() {
        "failed"
    } else if result.is_denied() {
        "denied"
    } else {
        "allowed"
    }
}

/// Outcome of the `UserPromptSubmit` lifecycle hook for a turn.
enum PromptSubmitDecision {
    Proceed,
    Denied { reason: Option<String> },
}

fn user_prompt_submit_denial_message(reason: Option<&str>) -> String {
    match reason.map(str::trim).filter(|reason| !reason.is_empty()) {
        Some(reason) => format!("user prompt blocked by UserPromptSubmit hook: {reason}"),
        None => "user prompt blocked by UserPromptSubmit hook".to_string(),
    }
}


/// Coordinates the model loop, tool execution, hooks, and session updates.
#[allow(clippy::struct_excessive_bools)] // each bool is an independent feature gate threaded from settings, not a state machine
pub struct ConversationRuntime<C, T> {
    session: Session,
    api_client: C,
    tool_executor: T,
    permission_policy: PermissionPolicy,
    system_prompt: Arc<[String]>,
    /// Per-turn harness reminders (todo progress, `TeamInbox` digest, hook
    /// context, …), toggled by [`Self::set_transient_system_reminder`] /
    /// [`Self::replace_transient_system_reminder_by_prefix`]. Sent as
    /// `ApiRequest::wire_reminders` and appended to the newest user-role wire
    /// message at lowering time — never to `system_prompt`, whose blocks sit
    /// in front of every message cache breakpoint: mutating system there
    /// re-billed the entire history (`system_changed`) each time a reminder
    /// refreshed.
    transient_reminders: Vec<String>,
    /// Slugs of memory entries whose full body is already persisted in this
    /// session's transcript. A re-appearing entry collapses to a pointer line
    /// at the absorb seam, since its body is still in context. Derived from
    /// the transcript by [`recalled_slugs_from_session`] — at construction, on
    /// a session swap, and after a compaction — so an entry reseeds in full
    /// exactly when its body stops being there.
    recalled_memory_slugs: HashSet<String>,
    /// The connected gateways (`providers[]` rows) whose content filter
    /// refused a request because of the memory recalled into it. For the rest
    /// of this session they are asked without recall, so the refusal costs one
    /// request, not one per turn. First-party models never land here.
    recall_withheld_gateways: HashSet<String>,
    /// Session-lifetime record of the check-shaped bash commands this session
    /// observed exiting 0 and the files edited after each of them, rendered
    /// into a turn-start observation by
    /// [`Self::inject_verified_state_reminder`] (see
    /// `conversation::verified_state`).
    ///
    /// Owned by the runtime rather than the tool context because the seam that
    /// CONSUMES it is a turn-start reminder, and because both recording seams
    /// (`record_turn_completed`) already sit here. Bound to a session sidecar
    /// by [`Self::reset_verified_state_session`], which the hosts call from the
    /// same places they bind the file-read registry — including inside
    /// `build_runtime`, since headless rebuilds a runtime every turn and an
    /// unbound ledger would be empty at exactly the stage boundary it exists
    /// to cross.
    verified_state: crate::verified_state::VerifiedStateLedger,
    /// The attempt this runtime is currently spending requests on — the key
    /// four ledgers join on (`crate::model_router::attempt`). Minted at every
    /// turn start by [`Self::begin_attempt`] and read by the recorders:
    /// `record_request_timing` stamps it on the timing row, and the deposit
    /// this runtime makes under [`Self::attempt_cache_scope`] puts it on the
    /// prompt-cache request rows. Empty until the first turn begins.
    attempt: String,
    /// The attempt a PARENT handed this runtime, if it is a spawned agent's:
    /// `<agentId>#<runGeneration>`, fixed for the whole run. A child that
    /// minted a turn key of its own would give its request rows a key the
    /// spawn record never carries, and the child's cost could not be added to
    /// the attempt that paid for it. `None` in the main session, which mints
    /// one turn key per user turn instead.
    inherited_attempt: Option<String>,
    /// The prompt-cache session id this runtime's requests are recorded under,
    /// when it is not [`Session::session_id`]. A spawned agent's cache is
    /// scoped to its AGENT id (`provider_client::prompt_cache_session_id`),
    /// while its `Session` carries an unrelated generated id — deposit under
    /// the wrong one and the request rows get no attempt at all.
    attempt_cache_scope: Option<String>,
    /// Query-aware persistent-memory retriever. Recalled entries are appended
    /// to the outgoing request's wire reminders only, so the base prompt and
    /// cacheable static prefix do not accumulate stale per-turn memory.
    ///
    /// `Arc` (not `Box`) so the streaming turn can `Arc::clone` it into a
    /// `spawn_blocking` recall — keeping the dense (ONNX) embedding forward pass
    /// off the drive-loop thread (FREEZE-1). `recall` takes `&self`, so the
    /// shared handle is a drop-in for the prior owned box.
    memory_retriever: Option<Arc<dyn MemoryRetriever + Send + Sync>>,
    /// Seated beside the retriever and shown every recall after it settles;
    /// its answer is the order the turn reads. See [`crate::RecallSeat`].
    recall_seat: Option<Arc<dyn crate::RecallSeat>>,
    /// Seated beside full compaction and asked, once per boundary, which
    /// tool results the summary still needs to read. See
    /// [`crate::CompactionSeat`].
    compaction_seat: Option<Arc<dyn crate::CompactionSeat>>,
    /// Seated beside the edit tools and asked, once per patch, what it makes
    /// of the patch before the model reads the result. See
    /// [`crate::PatchReviewSeat`].
    patch_review_seat: Option<Arc<dyn crate::PatchReviewSeat>>,
    max_iterations: usize,
    /// Optional wall-clock deadline for the turn. Two callers set it: spawned
    /// sub-agents bound a straggler that overran its caller's wait window, and
    /// the interactive host sets `now + turn wall-clock budget` each turn as a
    /// runaway circuit breaker (see `turn_output_token_budget`). Checked at
    /// iteration boundaries in both turn loops; `None` is unbounded.
    deadline: Option<std::time::Instant>,
    /// Progress-gated deadline-extension policy `(max_extensions, step)`. When
    /// the deadline passes but the turn produced fresh progress tool results
    /// since the last window, the streaming loop pushes the deadline out by
    /// `step` (at most `max_extensions` times per turn) instead of stopping
    /// mid-work. `None` (the default, and always for sub-agents) keeps the
    /// deadline a hard bound. Set per turn by the interactive host from
    /// `ZO_DEADLINE_EXTENSIONS` / `ZO_DEADLINE_EXTENSION_SECS`.
    deadline_extension: Option<(u8, std::time::Duration)>,
    /// Optional per-turn cumulative-output-token budget — the cost circuit
    /// breaker companion to [`Self::deadline`]. When a turn's output tokens
    /// (measured from turn start) cross this at an iteration boundary, the turn
    /// stops gracefully with [`BudgetExhausted::OutputTokens`] (work preserved,
    /// resumable), so an agentic loop that keeps generating without converging
    /// cannot silently burn tokens for hours. `None` is unbounded. Set by the
    /// interactive host each turn from `ZO_TURN_OUTPUT_TOKEN_BUDGET`.
    turn_output_token_budget: Option<u32>,
    /// Optional per-turn cumulative full-price-input-token budget — the third
    /// cost circuit breaker, covering the axis the other two miss: a
    /// cache-dead loop that re-sends the whole transcript uncached on every
    /// call burns millions of *input* tokens while generating little output
    /// and finishing well inside the wall clock. `input_tokens` excludes
    /// cache reads/writes on both provider normalizations, so a healthy
    /// cached turn stays far below any sane budget. `None` is unbounded. Set
    /// by the hosts each turn from `ZO_TURN_INPUT_TOKEN_BUDGET`.
    turn_input_token_budget: Option<u32>,
    max_tool_calls: usize,
    /// Upper bound on `TurnEnd`-hook continuations (Stop-loop). A `TurnEnd`
    /// hook can re-inject a `followupMessage` to keep the agent working; this
    /// caps how many times in a row that may happen before the turn returns.
    max_stop_loops: usize,
    usage_tracker: UsageTracker,
    hook_runner: HookRunner,
    auto_compaction_input_tokens_threshold: u32,
    precompaction_input_tokens_threshold: u64,
    context_policy: ContextPolicy,
    /// Settings `autoCompactThresholdPercent` override (already clamped by the
    /// config parser). Kept alongside `context_policy` — which it was folded
    /// into at construction — because a live `/model` switch rebuilds the
    /// policy from the model family (`set_context_model`) and must re-apply
    /// the user's ceiling instead of silently reverting to the family default.
    full_compaction_override_percent: Option<u8>,
    /// Total input context window for the active model (tokens).
    context_window: u64,
    hook_abort_signal: HookAbortSignal,
    /// "Cancel the tool, keep the turn" signal. Distinct from
    /// `hook_abort_signal`, which ends the whole turn — see
    /// [`crate::tool_cancel`].
    tool_cancel_signal: crate::tool_cancel::ToolCancelSignal,
    // `+ Send` so the whole `ConversationRuntime` is `Send`: `zo serve`
    // builds sessions on a `spawn_blocking` worker and shares them across the
    // multi-thread runtime's tasks. The two impls (`CliHookProgressReporter`,
    // the test recorder) are already `Send`; the bound is on the box type, not
    // the trait, so `&mut dyn HookProgressReporter` call sites are unaffected.
    hook_progress_reporter: Option<Box<dyn HookProgressReporter + Send>>,
    session_tracer: Option<SessionTracer>,
    async_api_client: Option<Arc<dyn AsyncApiClient>>,
    /// Optional dispatch function for parallel tool execution.
    /// When set, concurrency-safe tools (Read, Glob, Grep, etc.) are
    /// executed in parallel via `spawn_blocking` instead of sequentially.
    concurrent_dispatch: Option<ConcurrentDispatchFn>,
    /// User-facing auto-compaction gate. Emergency/request-building compaction
    /// remains available to avoid provider context-window failures.
    auto_compaction_enabled: bool,
    /// User-facing `TeamInbox` digest gate. Defaults on for compatibility; a
    /// max-updates value of 0 disables injection even when this is true.
    team_inbox_digest_enabled: bool,
    team_inbox_digest_max_updates: usize,
    /// User-facing gate for the input-triggered recall hint. Defaults on; when
    /// off, a past-reference cue never injects [`RECALL_HINT_REMINDER`].
    recall_hint_enabled: bool,
    /// True after the runtime has deferred preflight precompaction once to give
    /// `StateDistill` a request-visible working-state snapshot. Stored separately
    /// from the transient prompt because the prompt is cleared at turn start;
    /// this flag prevents repeatedly starving precompaction in long sessions.
    state_distill_deferred_precompaction: bool,
    /// One-shot latch for the pre-compaction early-warning line. `true` once the
    /// heads-up ("Context nearing auto-compaction — threshold M%…") has surfaced for the
    /// current context segment, so it fires exactly once as the session climbs
    /// toward the full-compaction ceiling instead of every turn. Re-armed
    /// (`false`) in `finish_compaction_swap` when a real compaction shrinks the
    /// transcript back below the ceiling, so the next approach warns again.
    precompaction_warned: bool,
    /// Per-runtime Dreamer automation gate. Natural candidate producers must use
    /// this field instead of reloading cwd config or touching process-global
    /// state, because one process may host multiple sessions with different
    /// feature configs.
    dream_automation_enabled: bool,
    /// Mid-turn steering messages typed by the user while a streaming turn is
    /// in flight. Drained at each tool-result boundary and folded into the
    /// last (tool-result) message as extra text blocks, so the user/assistant
    /// alternation the API requires is never broken. The TUI command pump
    /// pushes into this via [`ConversationRuntime::steering_handle`].
    steering: SteeringQueue,
    /// Told the exact steers a boundary just drained — the moment a message a
    /// parent posted was READ by this turn (t-2513 contract 2: the receipt
    /// moves from `queued` to `consumed` here and nowhere else). `None` for
    /// every host that does not report receipts.
    steering_observer: Option<SteeringObserver>,
    /// Told each model switch this runtime makes or lends out (see
    /// [`ModelSwitch`]). Set-or-cleared by the host at every turn entry;
    /// `None` means nobody is keeping score.
    switch_observer: Option<SwitchObserver>,
    /// Background-agent completions staged for mid-turn delivery while a turn
    /// is in flight. Drained at the same tool-result boundary as `steering`
    /// and folded as extra text blocks (CC's task-notification contract), so a
    /// main model that keeps working after spawning learns of finished agents
    /// without ending its turn. The host pushes via
    /// [`ConversationRuntime::agent_notification_inbox`] and re-queues
    /// whatever the turn never folded as follow-up turns.
    agent_notifications: AgentNotificationInbox,
    /// True when nobody can answer a mid-run question on this surface
    /// (headless one-shots). Turns the turn-end gate's question lint on; the
    /// promise lint runs regardless. Set by the host via
    /// [`Self::set_autonomous_surface`]; sub-agents never run the gate (it
    /// lives in the streaming loop only).
    autonomous_surface: bool,
    /// How many deep-lane sub-turns (PLAN/EXEC/VERIFY legs) are open on this
    /// runtime. Non-zero while a leg runs; the completion-receipt screen of
    /// the turn-end gate stands down inside one, because a leg's completion is
    /// judged by the VERIFY leg that follows it, not by a check of its own.
    deep_subturn_depth: u32,
    /// Tool names that must run via `spawn_blocking` even though they are not
    /// built-in long-running tools. Plugin-backed tools spawn a blocking
    /// subprocess (like `Bash`), so the host registers their names here to
    /// keep the TUI render loop from freezing during execution. See
    /// [`is_long_running`] for the built-in set.
    long_running_tool_names: BTreeSet<String>,
    /// Predicate marking *additional* tools as long-running, evaluated live so
    /// it tracks tools registered after construction — notably MCP server tools,
    /// whose blocking network RPC would otherwise freeze the TUI render loop.
    long_running_predicate: Option<LongRunningPredicate>,
    /// Rolling fingerprints of the most recent tool calls in the current turn.
    /// Used to detect a confused agent re-issuing an identical call and nudge
    /// it to break out. Cleared at each turn start; see
    /// [`Self::note_tool_repetition`].
    tool_fingerprint_counts: HashMap<u64, usize>,
    /// Fingerprints that emitted a per-turn advisory in the current assistant
    /// tool batch. Promoted after the batch is fully delivered so a same-batch
    /// duplicate does not hard-stop before the model can see the warning.
    tool_repetition_pending_hard_stop_fps: HashSet<u64>,
    /// Per-turn fingerprints whose advisory has already been delivered to the
    /// model in a prior tool batch and may therefore hard-stop on another repeat.
    tool_repetition_hard_stop_fps: HashSet<u64>,
    /// Successful `read_file` line ranges read during the current turn, keyed by
    /// path. Distinct, non-overlapping windows are progress; fully covered
    /// rereads are token leaks and get a range-aware advisory. They must not
    /// hard-stop: one redundant window in a multi-tool exploration batch should
    /// not cancel the remaining independent reads.
    read_file_ranges_by_path: HashMap<String, Vec<ReadFileRange>>,
    /// Paths whose covered-range reread advisory was already emitted this turn.
    /// Used only to avoid a wall of duplicate advisory text.
    read_file_redundant_advised_paths: HashSet<String>,
    /// `path → fingerprints of the non-mutating calls that touched it`.
    ///
    /// Exists so a successful edit can forgive the re-reads of *that file* without
    /// forgiving every other file's. Measured: across this repository's twelve
    /// longest sessions, 641 `read_file` calls repeated an identical input — about
    /// 6% of all tool calls — because every successful edit wiped the whole
    /// repetition tally, and a long editing session is nothing but successful
    /// edits. See [`Self::note_tool_repetition`].
    read_fingerprints_by_path: HashMap<String, HashSet<u64>>,
    /// Fingerprints of this turn's non-mutating calls that name no path —
    /// commands and searches. A successful mutation forgives them all: the
    /// same test command run again after an edit is a re-check, not a loop.
    /// Without this, a long turn that edits and re-runs its tests hit the
    /// per-turn hard stop on the fourth run ("`bash` called with identical
    /// input 4 times without making progress") with the work half done.
    pathless_fingerprints: HashSet<u64>,
    /// How many tool batches in a row held exactly one read-only call. The
    /// prompt asks for independent reads in one message; the day before r40
    /// had 83% of tool batches carrying a single call (694 of 837, counted by
    /// `docs/analysis/tools/tool-habits-r40.py`). At
    /// [`repetition::SERIAL_READS_NUDGE_AT`] the harness says so once, on the
    /// result the model is about to read. Reset with the other per-turn tallies.
    serial_read_batches: u8,
    /// Whether this turn already had its one nudge.
    serial_reads_nudged: bool,
    /// Who can stop this turn ([`Attendance`]). The repetition guard ends an
    /// unattended turn it judges stuck; an attended one only skips the repeat
    /// and says so — the person at the keyboard is the breaker there, the way
    /// it is in Claude Code and Codex.
    attendance: Attendance,
    /// Consecutive rounds on which the tier-1 microcompact fired without the
    /// session dropping back under its trim floor — the microcompact-thrashing
    /// signal (read files → clear their bodies → the model re-reads them). Reset
    /// when the pressure clears (context falls under the trim floor) or when a
    /// full compaction actually summarizes the transcript. At
    /// [`MICROCOMPACT_THRASH_PROMOTION`] the compaction gate promotes to full
    /// compaction so tool-result trimming can no longer starve the
    /// LLM-summarize path that ends the loop.
    consecutive_microcompacts: usize,
    /// Full compactions completed within the current turn. Ordinary turns see
    /// 0–1 (occasionally 2 on a giant context); a repetition loop that keeps
    /// inflating the transcript can trigger one per cycle — and every
    /// compaction used to clear ALL repetition state, so the loop restarted
    /// with a blank guard each round ("loop → inflate → compact → guard reset
    /// → same loop"). `finish_compaction_swap` consults this to stop clearing
    /// the repetition state past
    /// [`REPETITION_CLEAR_MAX_FULL_COMPACTIONS_PER_TURN`], letting the guard
    /// finally accumulate across cycles and trip. Reset at turn start.
    full_compactions_this_turn: usize,
    /// Cross-turn tally of identical (normalized) tool-call fingerprints. Unlike
    /// [`Self::tool_fingerprint_counts`] this is SESSION-scoped: it is NOT
    /// cleared at turn start, so a no-progress re-read loop that spans turn
    /// boundaries still accumulates and can trip the cross-turn advisory/hard
    /// stop (and the microcompact thrash-escape). Cleared when an edit/write
    /// tool records real progress and when a full compaction summarizes the
    /// transcript. See [`Self::note_tool_repetition`].
    cross_turn_tool_fingerprints: HashMap<u64, usize>,
    /// Cross-turn fingerprints that emitted an advisory in the current tool
    /// batch and become hard-stop eligible after that batch is delivered.
    cross_turn_tool_repetition_pending_hard_stop_fps: HashSet<u64>,
    /// Cross-turn fingerprints whose advisory was delivered in an earlier batch.
    cross_turn_tool_repetition_hard_stop_fps: HashSet<u64>,
    /// Per-turn tally of permission denials by (tool, audit-class) — the
    /// audit class is the "Permission audit: active mode is X; required mode
    /// is Y" sentence, which is identical across *different* inputs denied by
    /// the same mode. The first denial of a class keeps its full reason;
    /// later ones fold to one line (mode denials are deterministic, so a wall
    /// of repeats only bloats the transcript). Cleared at each turn start.
    mode_denial_counts: HashMap<(String, String), usize>,
    /// Set when [`Self::note_tool_repetition`] sees a tool call repeat
    /// [`TOOL_REPETITION_HARD_STOP`] times this turn: the turn loop ends the turn
    /// after the current tool batch rather than re-requesting the model straight
    /// back into the same no-progress loop. Reset at turn start.
    tool_loop_break_requested: bool,
    /// Consecutive verify-class rounds (`Workflow` / `WorkflowValidate` /
    /// `SpawnMultiAgent` / `Agent`) this turn that changed no file — the
    /// "verification treadmill" tally. A file-mutating batch resets it to 0; a
    /// pure research batch (read/grep/glob/bash) leaves it unchanged. At the soft
    /// threshold [`Self::note_verify_treadmill`] injects an advisory; at the hard
    /// threshold the turn stops gracefully with
    /// [`BudgetExhausted::VerificationTreadmill`]. Reset at turn start (both turn
    /// loops). See `verify_treadmill`.
    verify_treadmill_run: usize,
    /// Successful objective-progress tool results recorded this turn (see
    /// [`count_progress_tool_results`]). The one ledger BOTH behavior heuristics
    /// consult before ending a turn, so a nudge granted to one is charged
    /// against the same watermark as the other. Reset at turn start (both loops).
    turn_progress_results: usize,
    /// Nudges spent this turn: how many times a behavior heuristic was demoted
    /// from "end the turn" to "warn and keep going". Turn-scoped and SHARED
    /// across every heuristic, so the extra work one turn can win stays bounded
    /// by [`HEURISTIC_STOP_MAX_NUDGES`] no matter how many guards fire. Reset at
    /// turn start (both loops).
    heuristic_stop_nudges: u32,
    /// [`Self::turn_progress_results`] as of the last heuristic firing. A later
    /// heuristic is spared only when the turn progressed PAST this mark, so one
    /// early burst of edits cannot excuse an unbounded run of stops. Reset at
    /// turn start (both loops).
    progress_marker_at_last_signal: usize,
    /// Set when a turn ended on a harness budget/treadmill closer, so the NEXT
    /// turn carries the continuation reminder naming that closer as a harness
    /// cutoff rather than the model's own decision to quit. Deliberately NOT
    /// cleared by the per-turn reset (it is written at the end of one turn and
    /// read at the start of the next); consumed by
    /// [`Self::install_turn_budget_continuation_reminder`].
    budget_closer_pending: bool,
    /// When set, the tool the agent must call before the turn can end (workflow
    /// 8c). After the natural loop, if the agent has not called it, `run_turn`
    /// forces one final turn with `tool_choice = Tool { name }` so a schema
    /// phase always yields a captured `StructuredOutput` tool input. `None`
    /// (the default for the main loop and every free-text agent) is a no-op.
    structured_output_tool: Option<String>,
    /// Deep-lane gate config (plan → implement → verify → retry). `None` is an
    /// ordinary single-pass turn; `Some` means the host routes turns through
    /// [`Self::run_deep_turn_streaming`]. See `conversation/deep_gate.rs`.
    deep_gate: Option<DeepGateConfig>,
    /// Stable logical workspace directory for this session, used to root the
    /// durable external traces (`.zo/dream/`, `.zo/turns/`). `None` falls
    /// back to the live process cwd. This must be set when the process cwd can
    /// diverge from the workspace — e.g. `EnterWorktree` calls `set_current_dir`,
    /// and `zo serve` keeps multiple sessions alive across cwd changes — so
    /// the trace *producers* write to the same `.zo/` the *consumer*
    /// (auto-dream) later reads. See [`Self::set_workspace_cwd`].
    workspace_cwd: Option<std::path::PathBuf>,
    /// Last process working directory this runtime observed, for
    /// [`Self::fire_cwd_changed_if_moved`]. `None` until the first look.
    last_observed_cwd: Option<std::path::PathBuf>,
    /// Transcript length already scanned for `FileChanged`. Everything past it
    /// is a batch's new results; see [`Self::fire_file_changed_for_new_edits`].
    file_change_watermark: usize,
    /// Todo statuses as of this turn's entry, keyed by item text, for
    /// [`Self::fire_todo_lifecycle_hooks`]. Seeded at turn start so a plan
    /// written by the FIRST tool batch still reads as created.
    todo_baseline: std::collections::BTreeMap<String, crate::todo_progress::TodoStatus>,
    /// Pending `TeamInbox` deliveries that were injected into the current real
    /// user turn and must settle to `acked` or `failed` when the turn exits.
    /// None for denied prompts, internal subturns, degraded stores, or turns
    /// without unread inbox updates.
    team_inbox_turn: Option<TeamInboxDeliveryBatch>,
    /// Reasoning-effort floor (thinking budget, tokens) applied to outgoing
    /// requests, or `None` for the client default. Plumbed onto every
    /// [`ApiRequest`] as `effort_override` and treated as a floor by the client
    /// (`max(this, configured)`), so it can only *raise* effort. The deep-gate
    /// sets it on a stalled retry to power up a hard task that the starting
    /// effort could not solve; cleared at the end of the deep turn. `None` on an
    /// ordinary turn, so configured effort is unchanged unless escalation
    /// explicitly engages. See [`Self::set_effort_override`].
    effort_override: Option<u32>,
    /// The session's base model id (set by [`Self::set_context_model`] and at
    /// construction from the feature config). Kept so the loop can identify the
    /// active model family — the only reason the runtime needs the raw string,
    /// which `context_policy`/`context_window` do not retain. `None` when the
    /// host never told the runtime its model (bare test harnesses); the
    /// refusal-fallback path then stays inert. See [`Self::decide_refusal_fallback`].
    context_model: Option<String>,
    /// Per-turn model override plumbed onto every [`ApiRequest`] as
    /// `model_override`. `Some` means an Anthropic safety-classifier refusal
    /// (`stop_reason: "refusal"`) on a Fable/Mythos turn has been retried once on
    /// [`REFUSAL_FALLBACK_MODEL`] this turn — so it doubles as the "already
    /// fell back this turn" flag that caps the fallback at one. Reset at every
    /// turn start (`begin_turn_once` / `begin_streaming_turn`), so a fallback is
    /// scoped to the turn it fired on and never permanently swaps the model.
    refusal_fallback_model: Option<String>,
    /// Whether the current PUBLIC turn already spent its one same-model retry
    /// after a non-Fable classifier decline. See
    /// [`RefusalDecision::RetrySameModel`].
    refusal_same_model_retry_used: bool,
    /// Whether the current PUBLIC turn has armed [`Self::refusal_fallback_model`]
    /// at least once. Set only when [`Self::decide_refusal_fallback`] returns
    /// `Retry`; folded and cleared at the next public turn begin so internal
    /// deep-lane legs and auto-continuations never double-count the same user
    /// turn.
    refusal_turn_hit: bool,
    /// Number of consecutive completed PUBLIC turns that hit the refusal
    /// fallback. A clean completed turn resets this to zero when the next public
    /// turn begins. The running value lets the current turn enter refusal-dry as
    /// soon as it becomes the second consecutive refusal turn.
    refusal_consecutive_turns: u8,
    /// Session-scoped instant until which Fable/Mythos is presumed sticky on
    /// the accumulated context. While active, each leg that would otherwise run
    /// a Fable/Mythos model on the native client pre-arms the Opus family head;
    /// once elapsed,
    /// the next begin clears it and probes Fable normally. Process memory only,
    /// deliberately not persisted across restarts.
    refusal_dry_until: Option<std::time::Instant>,
    /// One-shot latch consumed by the turn loop to emit the refusal-dry warning
    /// before the first pre-armed request. Unlike quota pre-arm notices, later
    /// turns in the same refusal cooldown remain silent.
    refusal_prearm_notice_pending: bool,
    /// Whether this refusal-dry window has already latched its one pre-arm
    /// warning. Separate from `pending` so consuming the warning cannot cause a
    /// later dry turn begin to latch it again.
    refusal_prearm_notice_latched: bool,
    /// Per-turn wire-model ESCALATION plumbed onto [`ApiRequest`] as
    /// `model_override` (below the refusal fallback in precedence — a refusal
    /// on the escalated model must still swap to the safe fallback). Installed
    /// by the host at turn entry when the confidence cascade armed (the model
    /// verbalized low confidence at the end of the previous turn) and a
    /// same-provider Deep-tier model is routable; set-or-cleared every turn
    /// entry like [`Self::deep_verify_candidates`], so it can never outlive the
    /// one escalated turn. Same-provider only: this rides the bound client's
    /// wire model id, it never swaps clients. See
    /// [`Self::set_escalation_model_override`].
    escalation_model_override: Option<String>,
    /// One-shot freshness latch for [`Self::escalation_model_override`]:
    /// `set_escalation_model_override(Some)` arms it, and the FIRST turn
    /// begin after that consumes it (the escalated turn). A later turn begin
    /// without a fresh install clears the override instead — so a turn path
    /// that never runs the installing host code (a slash-command render, a
    /// queued-text turn on the same runtime) can never silently run on a
    /// stale Deep model from a previous turn's escalation.
    escalation_armed_fresh: bool,
    /// Cross-provider client the turn loop swaps to when the main model's
    /// subscription/quota window is exhausted — a `RateLimit` failure that
    /// survived the retry budget and would otherwise kill the turn. Carries the
    /// client plus its model id (for the notice). Installed by the host on every
    /// turn entry (mirrors [`Self::deep_verify_candidates`]): the top-ranked
    /// *different-provider* alternative for the main model, or `None` when Smart
    /// routing is off, the quota-fallback feature is disabled, the pool is
    /// single-provider, or the other provider's client cannot be built. `None`
    /// keeps the pre-feature behavior — a quota-exhausted turn fails as before.
    /// See [`Self::set_quota_fallback_client`].
    quota_fallback_client: Option<(Arc<dyn AsyncApiClient>, String)>,
    /// True while THIS turn is running on [`Self::quota_fallback_client`] rather
    /// than the native client. Set either when a mid-turn quota exhaustion swaps
    /// to the fallback, or at turn start when the session cooldown pre-arms it.
    /// Doubles as the one-shot cap: a fallback that is itself rate-limited ends
    /// the turn (no second fallback — mirrors the refusal one-shot cap), and it
    /// routes both the request dispatch and [`Self::effective_request_model`]
    /// through the fallback so the refusal path judges the *active* model. Reset
    /// at turn start unless the cooldown re-arms it. See
    /// [`Self::begin_turn_quota_fallback`] and [`Self::decide_quota_escape`].
    quota_fallback_active: bool,
    /// Session-scoped instant until which the main model is presumed
    /// quota-exhausted. Set when a fallback fires (the provider's `retry_after`
    /// hint if any, else [`QUOTA_FALLBACK_DEFAULT_COOLDOWN`]). While in the
    /// future the next turns pre-arm straight onto the fallback client without
    /// re-spending the main model's retry budget just to rediscover the wall is
    /// still up; once elapsed the session returns to the main model and this
    /// clears. Process memory only — deliberately NOT persisted across restarts
    /// (a fresh binary should re-probe the main model).
    quota_dry_until: Option<std::time::Instant>,
    /// One-shot latch set when a turn pre-arms onto the fallback from the
    /// session cooldown, consumed by the turn loop to emit the short pre-arm
    /// notice exactly once at the top of the turn (the mid-turn swap has its own
    /// louder warn line, so this only covers the "already cooling down" entry).
    quota_prearm_notice_pending: bool,
    /// How close to a quota window's reset the turn loop will HOLD on the main
    /// model instead of swapping to the cross-provider fallback. When a hard
    /// `RateLimit` fires and the exhausted window lifts within this band, the
    /// turn sleeps out the wait and re-requests on the SAME model — no provider
    /// swap, no cooldown recorded — so a session on its configured model isn't
    /// bumped to another provider for a wall that was about to clear anyway.
    /// `ZERO` disables the band (pure fallback, the pre-feature behavior). Set
    /// by the host from `smart.quotaWaitBandMinutes`. See
    /// [`Self::decide_quota_escape`].
    quota_wait_band: std::time::Duration,
    /// One-shot cap: set when a turn has already waited out a quota window once,
    /// so a window that 429s AGAIN right after its nominal reset (a lying header
    /// or a still-rolling window) falls through to the fallback instead of
    /// looping the wait forever. Reset at turn start.
    quota_waited_this_turn: bool,
    /// Lighter same-provider model this turn was demoted onto after the provider
    /// shed the heavier one (HTTP 529 / `overloaded_error`). Highest-precedence
    /// wire-model override for the rest of the turn — see
    /// [`Self::assemble_request`]'s `model_override`.
    ///
    /// Capacity shedding is tier-shaped, not account-shaped: measured on the
    /// reported failure, every Opus request was refused while a Haiku sub-agent on
    /// the SAME account succeeded in the same second. So the request that gets
    /// through is a lighter one, which is exactly what the error's own hint
    /// advises and what no layer used to do — the turn just died instead.
    overload_demotion_model: Option<String>,
    /// One-shot cap for [`Self::overload_demotion_model`]: one demotion per turn,
    /// so a lighter tier that is ALSO shed escalates to the cross-provider
    /// fallback rather than walking the ladder to nothing inside one turn.
    /// Reset at turn start.
    overload_demoted_this_turn: bool,
    /// True while a deep-gate VERIFY leg has swapped [`Self::async_api_client`]
    /// to a cross-model deep-lane client (see the
    /// `DeepSubturnPermissionGuard`). The verifier and planner run on their own
    /// clients regardless of the main model's quota state, so these flags
    /// suppress the quota-fallback override in
    /// [`Self::active_async_client`] / [`Self::effective_request_model`] for the
    /// duration of those legs — the turn-scoped client swaps compose instead
    /// of the quota fallback shadowing the verifier. Restored by the guard's
    /// `Drop`, so a cancelled verify leg cannot leave it stuck on.
    deep_verify_leg_active: bool,
    /// True while a PLAN leg uses [`Self::deep_plan_client`]. Kept separate
    /// from VERIFY because only VERIFY walks the ranked candidate ladder.
    deep_plan_leg_active: bool,
    /// Cross-model PLAN client installed by the host when the session model is
    /// not itself a reserved deep model. Under the Architect policy this keeps
    /// every PLAN leg on the configured deep-tier pool independently of
    /// EXEC-swap policy.
    deep_plan_client: Option<(Arc<dyn AsyncApiClient>, String)>,
    /// Architect invariant for PLAN/VERIFY: never fall back to a non-reserved
    /// native session model when a deep-lane client is unavailable.
    deep_tier_only: bool,
    /// Ordered Architect PLAN/VERIFY membership pool installed by the host
    /// from `smart.deepTierModels` on every turn entry.
    deep_tier_models: Vec<String>,
    /// Ordered cross-model VERIFY candidates for the deep gate, top-ranked
    /// first, each `(client, model)`. Built by the host from Smart Router
    /// ranking (configured verifier primary/role-override first, then the
    /// router's next available models), so a candidate that is itself hard
    /// rate-limited can fail over to the next *different-provider* candidate
    /// without a hardcoded fallback model. Set every turn entry alongside
    /// [`Self::deep_verify_candidates`] (which stays the top candidate for the
    /// phase-note label). Empty ⇒ VERIFY runs on the native main client, the
    /// pre-feature behavior. See [`Self::set_deep_verify_candidates`] and the
    /// deep gate's `verify_subturn`.
    deep_verify_candidates: Vec<(Arc<dyn AsyncApiClient>, String)>,
    /// Index into [`Self::deep_verify_candidates`] the current VERIFY leg's
    /// `DeepSubturnPermissionGuard` swaps in. `verify_subturn` sets it before
    /// each candidate attempt; the guard reads it to pick the client. Never
    /// outlives a leg (the guard's swap is restored on `Drop`).
    deep_verify_candidate_idx: usize,
    /// The verifier model that actually produced the verdict this deep turn,
    /// set by `verify_subturn` when a candidate streams successfully. Recorded
    /// into the summary/telemetry so the *successful* verifier is reported, not
    /// the first candidate that was rate-limited and skipped. Reset at the top
    /// of each deep turn.
    deep_verify_succeeded_model: Option<String>,
    /// Per-turn Architect execution contract (`smart.policy=architect`): the
    /// implementer client the deep gate's EXEC legs swap to when the host
    /// classified this turn implementation-shaped and the session main model
    /// is reserved for plan/verify duty. Installed by the host on every turn
    /// entry (mirrors [`Self::deep_verify_candidates`] — set-or-cleared, never
    /// outlives its turn); `None` keeps every leg native (pre-contract
    /// behavior). See [`Self::set_exec_contract`].
    exec_contract: Option<deep_gate::ExecContract>,
    /// True while a deep-gate EXEC leg has swapped [`Self::async_api_client`]
    /// to the contract implementer (mirrors [`Self::deep_verify_leg_active`]):
    /// suppresses the quota-fallback override for the duration of that leg so
    /// the two turn-scoped client swaps compose. Restored by the guard's
    /// `Drop`, so a cancelled leg cannot leave it stuck on.
    exec_impl_leg_active: bool,
    /// True while an Architect EXEC leg intentionally runs on the session's
    /// native client because `smart.execSwap` did not arm for the turn. This
    /// remains a defensive edit-gate exemption if a host manually arms the
    /// gate; it does not affect fallback or effective-model selection because
    /// no client swap occurred.
    exec_native_leg_active: bool,
    /// Host-installed per-turn Architect edit gate: `true` only while this
    /// turn's execution contract has a live swapped implementer. A direct
    /// workspace edit by the foreground planner is then denied with an
    /// instruction to use that EXEC/delegation path instead.
    /// Set or cleared on every turn entry like the other per-turn slots; the
    /// deep gate also clears it mid-turn when failure escalation hands
    /// implementation back to the native model. Default `false` — sub-agent
    /// and headless runtimes never arm it.
    reserved_edit_gate: bool,
    /// The step effort governor the host installed for this turn, with its
    /// counters: the effort each request carries is decided here per step
    /// (`conversation/step_effort.rs`). `None` — every host that never asked,
    /// sub-agents included — leaves the client's own effort on every request.
    step_effort: Option<step_effort::StepEffortState>,
    /// Per-turn routing band used to choose proportional deep-gate VERIFY
    /// depth. `None` is deliberately full verification for hosts that do not
    /// install routing metadata.
    verify_band: Option<(RouteTaskComplexity, RouteTaskRisk)>,
    /// Per-turn probed task intent, installed next to [`Self::verify_band`] —
    /// the same set-every-turn-entry slot, since it answers the same question
    /// for the same consumer: what the deep gate's VERIFY leg should look at.
    /// [`RouteTaskIntent::Design`] arms the VERIFY design lens and floors the
    /// proportional depth at one lens; every other value (and every host that
    /// never installs one — headless, serve, sub-agents) keeps the exact
    /// pre-intent behavior. The design kill switch is applied by the host
    /// before this is set, so `off` simply never delivers `Design` here.
    verify_intent: RouteTaskIntent,
}

struct PreparedSyncTool<'a> {
    tool_use_id: String,
    tool_name: String,
    effective_input: Cow<'a, str>,
    pre_hook_result: HookRunResult,
    permission_outcome: PermissionOutcome,
}


enum EmptyAssistantAction {
    Retry,
    ContinueOnce,
    Exhausted,
}

fn pre_hook_denial_outcome(
    pre_hook_result: &HookRunResult,
    tool_name: &str,
) -> Option<PermissionOutcome> {
    if pre_hook_result.is_cancelled() {
        Some(PermissionOutcome::Deny {
            reason: format_hook_message(
                pre_hook_result,
                &format!("PreToolUse hook cancelled tool `{tool_name}`"),
            ),
        })
    } else if pre_hook_result.is_failed() {
        Some(PermissionOutcome::Deny {
            reason: format_hook_message(
                pre_hook_result,
                &format!("PreToolUse hook failed for tool `{tool_name}`"),
            ),
        })
    } else if pre_hook_result.is_denied() {
        Some(PermissionOutcome::Deny {
            reason: format_hook_message(
                pre_hook_result,
                &format!("PreToolUse hook denied tool `{tool_name}`"),
            ),
        })
    } else {
        None
    }
}

/// The waves a batch runs in: maximal runs of consecutive tools that are
/// allowed and ride a wave (reads, spawns), two or more long, in model order.
/// An ordered tool — Bash, Edit — or a denied one ends a run and runs alone at
/// its own place in the order, so `Edit -> Read` observes the edit and
/// `Bash -> Agent` starts after the shell. A wave runs when the serial pass
/// reaches its first tool, never earlier; the caller caps each wave at
/// [`MAX_PARALLEL_SAFE_TOOL_DISPATCHES`].
fn parallel_waves<'a>(
    tools: impl IntoIterator<Item = (usize, &'a str, &'a PermissionOutcome)>,
) -> Vec<Vec<usize>> {
    let mut waves = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for (idx, tool_name, outcome) in tools {
        if matches!(outcome, PermissionOutcome::Allow) && rides_a_wave(tool_name) {
            run.push(idx);
        } else if run.len() >= 2 {
            waves.push(std::mem::take(&mut run));
        } else {
            run.clear();
        }
    }
    if run.len() >= 2 {
        waves.push(run);
    }
    waves
}

fn take_truncation_continuation(
    stop_reason: Option<&str>,
    truncation_continuations: &mut usize,
) -> bool {
    if stop_reason.is_some_and(is_truncation_stop_reason)
        && *truncation_continuations < MAX_TRUNCATION_CONTINUATIONS
    {
        *truncation_continuations += 1;
        true
    } else {
        false
    }
}

fn tool_result_message(
    tool_use_id: &str,
    tool_name: &str,
    output: String,
    is_error: bool,
    images: Vec<(String, String)>,
) -> ConversationMessage {
    if images.is_empty() {
        ConversationMessage::tool_result(tool_use_id, tool_name, output, is_error)
    } else {
        ConversationMessage::tool_result_with_images(tool_use_id, tool_name, output, is_error, images)
    }
}


/// The transient channel a fresh runtime starts with.
///
/// If this runtime is built from an already-compacted session (a cold
/// `--resume`), re-inject the compaction reminder: the live one added by
/// `finish_auto_compaction` lived only in memory and is gone after a restart,
/// so without this the resumed model has no idea its context was compacted —
/// nor that the pre-compaction detail is recoverable from the vault via
/// `session_recall` (LAVA cross-session no-loss). Seeded into the transient
/// channel (not `system_prompt`) so a later `finish_compaction_swap` round
/// replaces it instead of duplicating it, and the system blocks stay frozen for
/// the prefix cache.
fn resume_compaction_reminders(session: &Session) -> Vec<String> {
    if session.compaction.is_some() {
        vec![COMPACTION_RESUME_REMINDER.to_string()]
    } else {
        Vec::new()
    }
}

/// Rebuild the recall dedup state from a transcript: the slugs whose FULL
/// body is persisted in it.
///
/// The state has to be derived rather than carried, because a runtime outlives
/// the session it was built with. A cold `/resume` builds a new runtime over a
/// transcript that already holds its bodies (starting empty would re-inject
/// them), while `/new`, fast resume and session switching swap a transcript
/// holding none into a live runtime (keeping the old set would leave a pointer
/// aimed at a body no longer in context). Reading the transcript answers both,
/// and answers compaction too: whatever the preserved tail kept is still there.
fn recalled_slugs_from_session(session: &Session) -> HashSet<String> {
    let mut seen = HashSet::new();
    for message in session.messages.iter() {
        if message.role != MessageRole::System {
            continue;
        }
        for block in &message.blocks {
            if let ContentBlock::Text { text } = block {
                crate::memory::recall::collect_recalled_full_entry_slugs(text, &mut seen);
            }
        }
    }
    seen
}

fn is_edit_or_write_tool(tool_name: &str) -> bool {
    // Single source of truth shared with the microcompact trim
    // (`compact::is_edit_result_tool`): both layers must agree on exactly which
    // tools record a file mutation, or one could trim a result the other counts
    // as an edit. See `EDIT_RESULT_TOOL_NAMES`.
    crate::compact::is_edit_result_tool(tool_name)
}

impl<C, T> ConversationRuntime<C, T> {
    /// Fold one completed tool result into this turn's objective-progress
    /// ledger — the single source of truth both behavior heuristics read.
    ///
    /// Progress is deliberately narrow (successful `edit_file` / `write_file` /
    /// `NotebookEdit` / `TodoWrite`, per [`count_progress_tool_results`]):
    /// reads and bash probes are exactly what a non-converging grind produces in
    /// bulk, so they must never buy it a reprieve.
    pub(super) fn note_progress_tool_result(&mut self, result: &ConversationMessage) {
        self.turn_progress_results = self
            .turn_progress_results
            .saturating_add(count_progress_tool_results(std::slice::from_ref(result)));
    }

    /// Decide whether a fired *behavior heuristic* ends the turn.
    ///
    /// A heuristic (verification treadmill, tool-call repetition) infers that the
    /// turn is stuck from a proxy signal, and the proxy misfires on real work —
    /// an orchestrator that delegates every edit looks exactly like a treadmill.
    /// So a heuristic that fires on a turn which has produced fresh objective
    /// progress spends the turn's single nudge and keeps going; everything else
    /// stops. Resource breakers never route through here (`decision_core` rejects
    /// them outright), so the wall clock and the token budgets stay absolute.
    pub(super) fn resolve_heuristic_stop(&mut self, signal: TurnStopSignal) -> TurnContinuity {
        let progressed = self.turn_progress_results > self.progress_marker_at_last_signal;
        let continuity = decide_turn_continuity(
            signal,
            progressed,
            self.heuristic_stop_nudges,
            HEURISTIC_STOP_MAX_NUDGES,
        );
        // Advance the watermark on EVERY firing, nudge or not: the next heuristic
        // must be paid for with progress made after this one, so a single early
        // burst of edits cannot excuse an unbounded run of stops.
        self.progress_marker_at_last_signal = self.turn_progress_results;
        if continuity == TurnContinuity::Nudge {
            self.heuristic_stop_nudges = self.heuristic_stop_nudges.saturating_add(1);
        }
        continuity
    }

    /// Bind the verified-state ledger to a session sidecar
    /// (`<session>.verified-state.json`) and restore whatever it recorded.
    ///
    /// The mirror of `ToolContext::reset_file_reads_session`, and it must be
    /// called from the same places for the same reason: a multi-stage bench run
    /// is a chain of PROCESSES resuming one session, and headless rebuilds the
    /// runtime on every turn. An unbound ledger is therefore empty precisely at
    /// the stage boundary the observation exists to cross. `None` unbinds
    /// (in-memory only — sub-agent runtimes), which is also the default, so a
    /// host that never binds it degrades to byte-neutral rather than to a wrong
    /// observation.
    ///
    /// Rebinding is also how a conversation RESET drops the history: `/clear`
    /// swaps in a fresh session and rebinds here, so the new conversation reads
    /// its own (empty) sidecar. There is deliberately no "wipe" entry point —
    /// at `/clear` time the ledger is still bound to the PREVIOUS session's
    /// sidecar, so a write-through wipe would destroy the history of the
    /// session `/clear` exists to keep resumable (the same trap already
    /// documented at the read-registry swap in `live_cli_commands`).
    pub fn reset_verified_state_session(&mut self, sidecar: Option<std::path::PathBuf>) {
        self.verified_state.rebind_sidecar(sidecar);
    }

    /// Install this turn's routing band for proportional deep-gate VERIFY.
    pub fn set_verify_band(
        &mut self,
        complexity: RouteTaskComplexity,
        risk: RouteTaskRisk,
    ) {
        self.verify_band = Some((complexity, risk));
    }

    /// Install this turn's probed task intent for the deep-gate VERIFY leg
    /// (set-or-cleared every turn entry, exactly like [`Self::set_verify_band`]
    /// — pass [`RouteTaskIntent::Other`] on a turn that arms nothing rather
    /// than leaving a prior turn's `Design` standing).
    ///
    /// The host is the kill-switch seam: it must map `Design` to `Other` when
    /// the user disabled design guidance, so this slot always means "the design
    /// lens is entitled to run".
    pub fn set_verify_intent(&mut self, intent: RouteTaskIntent) {
        self.verify_intent = intent;
    }

    /// The task intent installed for this turn's VERIFY lens selection.
    #[must_use]
    pub fn verify_intent(&self) -> RouteTaskIntent {
        self.verify_intent
    }

    /// Architect edit gate (`smart.policy=architect`): the denial reason when a
    /// live swapped implementer owns this turn's workspace mutation and a
    /// foreground tool call must be redirected to that EXEC path, `None` when
    /// the call may proceed.
    ///
    /// Exemptions, in order: no live implementer swap; an Architect EXEC leg;
    /// a deep-verify leg (its client never edits, and `ReadOnly` covers it
    /// anyway); and any leg whose active permission mode already forbids writes
    /// — the mode's own denial message is clearer there. Checked in BOTH tool-
    /// authorization paths (streaming and sync) ahead of the permission policy,
    /// so the instruction reaches the model as an ordinary denied tool result
    /// it can act on.
    fn architect_edit_gate_denial(&self, tool_name: &str) -> Option<String> {
        if !self.reserved_edit_gate
            || self.exec_impl_leg_active
            || self.exec_native_leg_active
            || self.deep_verify_leg_active
            || !is_edit_or_write_tool(tool_name)
            || !self
                .permission_policy
                .active_mode()
                .satisfies(PermissionMode::WorkspaceWrite)
        {
            return None;
        }
        Some(format!(
            "architect policy: this turn has a swapped implementer EXEC leg armed — route \
             `{tool_name}` through that EXEC/delegation path instead of editing from the \
             foreground planner."
        ))
    }
}


/// Shared queue of mid-turn steering messages. A cloneable handle so the TUI
/// command pump can push while the streaming turn holds `&mut self`.
pub type SteeringQueue = Arc<Mutex<Vec<String>>>;

/// Told the steers a boundary drained, in the order they were read. Installed
/// by a host that reports receipts ([`ConversationRuntime::with_steering_observer`]).
pub type SteeringObserver = Arc<dyn Fn(&[String]) + Send + Sync>;

/// One model change on the wire, as the runtime saw it happen: which door it
/// came through, which model it left and which it went to, how much context
/// the first request on the new model has to rewrite, and the attempt that
/// pays for it. Told to the [`SwitchObserver`] a host installs
/// ([`ConversationRuntime::set_switch_observer`]); the runtime records
/// nothing itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSwitch {
    pub trigger: crate::SwitchTrigger,
    pub from: String,
    pub to: String,
    /// The conversation the new model reads, as the runtime estimates it at
    /// the moment of the switch — for an isolated leg, what the leg sends.
    pub context_tokens: u64,
    /// The attempt key the switched requests will bill to.
    pub attempt: String,
}

/// Told each model switch the runtime makes or lends out — the quota and
/// refusal fallbacks, the overload demotion, and a deep-gate leg's client
/// swap. Installed set-or-clear at every turn entry by the host that files
/// the plan scorer's shadow rows; `None` for every other host.
pub type SwitchObserver = Arc<dyn Fn(&ModelSwitch) + Send + Sync>;

/// What a staged [`AgentNotification`] actually reports. The two kinds share
/// one inbox so the ORDER a sub-agent's mid-run messages and its final
/// completion reach the main model is the order they happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentNotificationKind {
    /// A background agent reached a terminal state (CC's task-notification).
    Completion,
    /// A still-RUNNING sub-agent pushed a message upstream to the parent
    /// conversation. Carries the sender's agent id so the host can cap how
    /// many of its messages may sit pending in one turn (a runaway agent must
    /// not be able to flood the main context).
    Message { agent_id: String },
}

/// A sub-agent event staged for **mid-turn delivery** to the main model (CC's
/// task-notification contract). `text` is the full model-facing message built
/// by the host at event time; `label`/`status` survive so a notification the
/// turn never reached a boundary to fold can be re-queued as a
/// follow-up-turn agent-result card instead.
#[derive(Debug, Clone)]
pub struct AgentNotification {
    /// Sub-agent display label (e.g. `runtime-scout`).
    pub label: String,
    /// Terminal status driving the card tint on both delivery paths.
    pub status: crate::message_stream::AgentResultStatus,
    /// Full model-facing message: for a completion the
    /// `[background agent … finished …]` header followed by the (elided)
    /// result body; for a message the teammate framing built by the host.
    pub text: String,
    /// Completion vs mid-run message — drives the preamble in
    /// `agent_notification_text` and the host's per-agent pending cap.
    pub kind: AgentNotificationKind,
    /// What the finished helper's run cost, already spelled by
    /// `core_types::helper_run` — the card's `Done (…)` line. `None` for a
    /// mid-run message and for a completion nothing was measured for.
    pub summary: Option<String>,
}

/// Shared inbox of background-agent completions awaiting mid-turn delivery.
/// The host's completion consumer pushes while the turn holds `&mut self`
/// (same shape as [`SteeringQueue`]); the turn drains at each tool-result
/// boundary. Whatever is still here when the turn ends is the host's to
/// re-queue as follow-up turns — exactly-once by construction: one inbox, and
/// the two drain points never run concurrently.
pub type AgentNotificationInbox = Arc<Mutex<Vec<AgentNotification>>>;

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    #[must_use]
    pub fn new(
        session: Session,
        api_client: C,
        tool_executor: T,
        permission_policy: PermissionPolicy,
        system_prompt: Vec<String>,
    ) -> Self {
        Self::new_with_features(
            session,
            api_client,
            tool_executor,
            permission_policy,
            system_prompt,
            &RuntimeFeatureConfig::default(),
        )
    }

    #[must_use]
    pub fn new_with_features(
        session: Session,
        api_client: C,
        tool_executor: T,
        permission_policy: PermissionPolicy,
        system_prompt: Vec<String>,
        feature_config: &RuntimeFeatureConfig,
    ) -> Self {
        let context_window = feature_config
            .model()
            .map_or(200_000, ::api::context_window_for_model);
        Self::new_with_context_window(
            session,
            api_client,
            tool_executor,
            permission_policy,
            system_prompt,
            feature_config,
            context_window,
        )
    }

    /// Like [`Self::new_with_features`] but accepts an explicit
    /// `context_window` so callers that know the real selected model
    /// can pass the correct value instead of relying on
    /// `feature_config.model()` which may be `None`.
    #[must_use]
    // The length is a flat field list, not branching logic: every line is one
    // slot of a struct whose fields are independent feature gates.
    #[allow(clippy::too_many_lines)]
    pub fn new_with_context_window(
        session: Session,
        api_client: C,
        tool_executor: T,
        permission_policy: PermissionPolicy,
        system_prompt: Vec<String>,
        feature_config: &RuntimeFeatureConfig,
        context_window: u64,
    ) -> Self {
        let usage_tracker = UsageTracker::from_session(&session);
        let full_compaction_override_percent = feature_config.auto_compact_threshold_percent();
        let context_policy = ContextPolicy::for_model(feature_config.model())
            .with_full_compaction_override(full_compaction_override_percent);
        let transient_reminders = resume_compaction_reminders(&session);
        let recalled_memory_slugs = recalled_slugs_from_session(&session);
        Self {
            session,
            api_client,
            tool_executor,
            permission_policy,
            system_prompt: Arc::from(system_prompt),
            transient_reminders,
            recalled_memory_slugs,
            recall_withheld_gateways: HashSet::new(),
            verified_state: crate::verified_state::VerifiedStateLedger::new(),
            attempt: String::new(),
            inherited_attempt: None,
            attempt_cache_scope: None,
            memory_retriever: None,
            recall_seat: None,
            compaction_seat: None,
            patch_review_seat: None,
            max_iterations: default_max_iterations(),
            deadline: None,
            deadline_extension: None,
            turn_output_token_budget: None,
            turn_input_token_budget: None,
            max_tool_calls: usize::MAX,
            max_stop_loops: DEFAULT_MAX_STOP_LOOPS,
            usage_tracker,
            hook_runner: HookRunner::from_feature_config(feature_config),
            auto_compaction_input_tokens_threshold: auto_compaction_threshold_from_env_or_policy(
                compaction::auto_compaction_window(context_window),
                context_policy,
            ),
            precompaction_input_tokens_threshold: context_policy.precompaction_threshold(
                compaction::auto_compaction_window(context_window)
                    .max(u64::from(FALLBACK_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD)),
            ),
            context_policy,
            full_compaction_override_percent,
            context_window,
            hook_abort_signal: HookAbortSignal::default(),
            tool_cancel_signal: crate::tool_cancel::ToolCancelSignal::default(),
            hook_progress_reporter: None,
            session_tracer: None,
            async_api_client: None,
            concurrent_dispatch: None,
            auto_compaction_enabled: feature_config.auto_compact_enabled(),
            team_inbox_digest_enabled: feature_config.team_inbox_digest_enabled(),
            team_inbox_digest_max_updates: feature_config.team_inbox_digest_max_updates(),
            recall_hint_enabled: feature_config.recall_hint_enabled(),
            state_distill_deferred_precompaction: false,
            precompaction_warned: false,
            dream_automation_enabled: feature_config.dream_automation_enabled(),
            steering: Arc::new(Mutex::new(Vec::new())),
            steering_observer: None,
            switch_observer: None,
            agent_notifications: Arc::new(Mutex::new(Vec::new())),
            autonomous_surface: false,
            deep_subturn_depth: 0,
            long_running_tool_names: BTreeSet::new(),
            long_running_predicate: None,
            tool_fingerprint_counts: HashMap::new(),
            tool_repetition_pending_hard_stop_fps: HashSet::new(),
            tool_repetition_hard_stop_fps: HashSet::new(),
            read_file_ranges_by_path: HashMap::new(),
            read_file_redundant_advised_paths: HashSet::new(),
            read_fingerprints_by_path: HashMap::new(),
            pathless_fingerprints: HashSet::new(),
            serial_read_batches: 0,
            serial_reads_nudged: false,
            attendance: Attendance::Unattended,
            consecutive_microcompacts: 0,
            full_compactions_this_turn: 0,
            cross_turn_tool_fingerprints: HashMap::new(),
            cross_turn_tool_repetition_pending_hard_stop_fps: HashSet::new(),
            cross_turn_tool_repetition_hard_stop_fps: HashSet::new(),
            mode_denial_counts: HashMap::new(),
            tool_loop_break_requested: false,
            verify_treadmill_run: 0,
            turn_progress_results: 0,
            heuristic_stop_nudges: 0,
            progress_marker_at_last_signal: 0,
            budget_closer_pending: false,
            structured_output_tool: None,
            deep_gate: None,
            workspace_cwd: None,
            last_observed_cwd: None,
            file_change_watermark: 0,
            todo_baseline: std::collections::BTreeMap::new(),
            team_inbox_turn: None,
            effort_override: None,
            context_model: feature_config.model().map(str::to_string),
            refusal_fallback_model: None,
            refusal_same_model_retry_used: false,
            refusal_turn_hit: false,
            refusal_consecutive_turns: 0,
            refusal_dry_until: None,
            refusal_prearm_notice_pending: false,
            refusal_prearm_notice_latched: false,
            escalation_model_override: None,
            escalation_armed_fresh: false,
            quota_fallback_client: None,
            quota_fallback_active: false,
            quota_dry_until: None,
            quota_prearm_notice_pending: false,
            quota_wait_band: std::time::Duration::ZERO,
            quota_waited_this_turn: false,
            overload_demotion_model: None,
            overload_demoted_this_turn: false,
            deep_verify_leg_active: false,
            deep_verify_candidates: Vec::new(),
            deep_verify_candidate_idx: 0,
            deep_verify_succeeded_model: None,
            deep_plan_leg_active: false,
            deep_plan_client: None,
            deep_tier_only: false,
            deep_tier_models: crate::default_deep_tier_models(),
            exec_contract: None,
            exec_impl_leg_active: false,
            exec_native_leg_active: false,
            reserved_edit_gate: false,
            step_effort: None,
            verify_band: None,
            verify_intent: RouteTaskIntent::Other,
        }
    }



    fn sync_parallel_batch_has_repetition_risk(&self, prepared: &[PreparedSyncTool<'_>]) -> bool {
        self.parallel_batch_has_repetition_risk(prepared.iter().map(|p| {
            (
                p.tool_name.as_str(),
                p.effective_input.as_ref(),
                &p.permission_outcome,
            )
        }))
    }






    /// Apply the `UserPromptSubmit` output policy at the turn boundary. The hook
    /// may block the turn or contribute low-trust context, but it must not rewrite
    /// the user's actual message (`updatedInput` remains parsed-but-ignored).
    fn apply_user_prompt_submit_hook(&mut self, user_input: &str) -> PromptSubmitDecision {
        if self
            .hook_runner
            .lifecycle_command_count(HookEvent::UserPromptSubmit)
            == 0
        {
            return PromptSubmitDecision::Proceed;
        }

        let result = self.run_lifecycle_hook(
            HookEvent::UserPromptSubmit,
            &json!({"user_input": user_input}),
        );
        if result.is_denied() {
            return PromptSubmitDecision::Denied {
                reason: result
                    .denial_reason()
                    .map(str::to_owned)
                    .or_else(|| result.messages().first().cloned()),
            };
        }
        if result.is_failed() || result.is_cancelled() {
            return PromptSubmitDecision::Proceed;
        }

        if let Some(reminder) =
            build_user_prompt_hook_context_reminder(result.additional_context_messages())
        {
            self.replace_transient_system_reminder_by_prefix(
                USER_PROMPT_HOOK_CONTEXT_REMINDER_PREFIX,
                Some(&reminder),
            );
        }
        PromptSubmitDecision::Proceed
    }

    /// Run the streaming user-entry lifecycle policy without recording telemetry
    /// or pushing a user message. Deep-mode orchestration calls this once for the
    /// outer, user-submitted prompt before its program-generated internal subturns;
    /// those subturns still call [`Self::begin_streaming_turn`] with
    /// `internal_subturn = true`, so they record their own subturn telemetry but do
    /// not re-run `TurnStart`/`UserPromptSubmit`.
    pub(crate) fn run_user_prompt_submit_for_streaming_user_entry(
        &mut self,
        user_input: &str,
    ) -> Result<(), StreamingTurnError> {
        let _ = self.run_lifecycle_hook(
            HookEvent::TurnStart,
            &json!({"session_id": self.session.session_id}),
        );
        self.clear_turn_start_transient_reminders();
        self.seed_todo_baseline();
        match self.apply_user_prompt_submit_hook(user_input) {
            PromptSubmitDecision::Proceed => {
                self.inject_team_inbox_digest_reminder();
                self.inject_recall_hint_reminder(user_input);
                self.inject_goal_clarify_reminder(user_input);
                // Every streaming entry point — TUI and all three headless
                // output formats — funnels through here, so this one call
                // covers the whole streaming dispatcher.
                self.inject_verified_state_reminder();
                self.install_turn_budget_continuation_reminder();
                Ok(())
            }
            PromptSubmitDecision::Denied { reason } => Err(StreamingTurnError::runtime(
                user_prompt_submit_denial_message(reason.as_deref()),
            )),
        }
    }

    /// The cumulative usage this runtime was rehydrated with from its
    /// session's stored history — zero for a fresh session. A per-run report
    /// (headless `--output-format json`) subtracts this from
    /// [`TurnSummary::usage`] (which is session-cumulative) so a resumed
    /// stage bills only its own consumption; without the subtraction every
    /// stage re-reports the whole session and a consumer that sums runs
    /// double-counts.
    #[must_use]
    pub fn usage_baseline(&self) -> TokenUsage {
        self.usage_tracker.rehydrated_usage()
    }

    /// Run a user turn, honoring the `TurnEnd` (Stop) hook's `followupMessage`:
    /// when a `TurnEnd` hook returns one, the message is re-injected as the next
    /// user turn and the agent keeps working, bounded by `max_stop_loops`. With
    /// no `followupMessage` this runs exactly one turn (the historical
    /// behavior). The returned summary is the final turn's; token usage is
    /// cumulative across continuations.
    pub fn run_turn(
        &mut self,
        user_input: impl Into<String>,
        mut prompter: Option<&mut dyn PermissionPrompter>,
    ) -> Result<TurnSummary, RuntimeError> {
        let mut input = user_input.into();
        // Keep the original request verbatim for the spec-literal self-verify
        // gate below — the Stop-loop rewrites `input` with each followup.
        let original = input.clone();
        let mut loop_count = 0;
        // Baseline for the WHOLE turn's output delta. Each `run_turn_once` leg
        // measures only its own delta, but a Stop-loop (TurnEnd followup) turn has
        // several legs; the returned summary is the last leg's, so re-derive the
        // turn total here (cumulative is monotonic within this runtime instance).
        let turn_base_output = self.usage_tracker.cumulative_usage().output_tokens;
        let result = loop {
            let (mut summary, followup) = match self.run_turn_once(input, &mut prompter, loop_count) {
                Ok(value) => value,
                Err(error) => break Err(error),
            };
            match followup {
                Some(followup) if loop_count < self.max_stop_loops => {
                    loop_count += 1;
                    input = followup;
                }
                _ => {
                    // The agentic turn stopped. Run the spec-literal self-verify
                    // gate (once — this arm always returns): if an edit reproduced
                    // a task-specified literal with the wrong case, patch it on
                    // disk directly (deterministic — a model repair only fixes the
                    // casing ~50% of the time, measured).
                    if turn_end::spec_literal_autopatch(&original) {
                        eprintln!("[zo] spec-literal gate: auto-patched exact-case literal(s)");
                    }
                    summary.turn_output_tokens = self
                        .usage_tracker
                        .cumulative_usage()
                        .output_tokens
                        .saturating_sub(turn_base_output);
                    break Ok(summary);
                }
            }
        };
        self.settle_team_inbox_turn_for_result(&result);
        result
    }


    #[allow(clippy::too_many_arguments)]
    fn build_turn_summary(
        &self,
        assistant_messages: Vec<ConversationMessage>,
        tool_results: Vec<ConversationMessage>,
        prompt_cache_events: Vec<PromptCacheEvent>,
        iterations: usize,
        auto_compaction: Option<AutoCompactionEvent>,
        microcompact: Option<crate::MicrocompactEvent>,
        turn_start_output_tokens: u32,
        budget_exhausted: Option<BudgetExhausted>,
    ) -> TurnSummary {
        TurnSummary {
            assistant_messages,
            tool_results,
            prompt_cache_events,
            iterations,
            usage: self.usage_tracker.cumulative_usage(),
            turn_output_tokens: self
                .usage_tracker
                .cumulative_usage()
                .output_tokens
                .saturating_sub(turn_start_output_tokens),
            auto_compaction,
            microcompact,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted,
        }
    }

    /// Turn-start prologue for [`Self::run_turn_once`]: fire the `TurnStart` and
    /// `UserPromptSubmit` lifecycle hooks, record turn-started telemetry, push
    /// the user input onto the session, and reset the per-turn transient state
    /// (recent-tool fingerprints + stale empty-stream / todo-progress reminders).
    ///
    /// Hook denial happens before telemetry/session recording so blocked prompts
    /// are not persisted or traced as `turn_started`.
    ///
    /// Returns the cumulative output-token count captured *before* this turn so
    /// the caller can compute `TurnSummary.turn_output_tokens` as a delta. Split
    /// out so the agentic loop body in `run_turn_once` reads as the turn's core
    /// rather than opening with a wall of setup.
    fn begin_turn_once(
        &mut self,
        user_input: String,
        is_continuation: bool,
    ) -> Result<u32, RuntimeError> {
        if !is_continuation { self.tool_executor.begin_user_turn(); }
        let _ = self.run_lifecycle_hook(
            HookEvent::TurnStart,
            &json!({"session_id": self.session.session_id}),
        );
        self.clear_turn_start_transient_reminders();
        self.seed_todo_baseline();
        match self.apply_user_prompt_submit_hook(&user_input) {
            PromptSubmitDecision::Proceed => {}
            PromptSubmitDecision::Denied { reason } => {
                return Err(RuntimeError::new(user_prompt_submit_denial_message(
                    reason.as_deref(),
                )));
            }
        }
        self.inject_team_inbox_digest_reminder();
        self.inject_recall_hint_reminder(&user_input);
        self.inject_goal_clarify_reminder(&user_input);
        // The sync loop gets the observation too. It is the deep-gate-less
        // path, so it was eligible to be deferred — but the injection is one
        // call on a seam that already exists here, and a session that mixes
        // both loops (a sub-agent turn, `zo -p` with `--output-format text`
        // on the sync fallback) would otherwise record facts it can never
        // report.
        self.inject_verified_state_reminder();
        self.install_turn_budget_continuation_reminder();
        // Before the user message is pushed: the ordinal this mints counts the
        // turns already taken, and this one is the next.
        self.begin_attempt();
        self.record_turn_started(&user_input);
        let turn_start_output_tokens = self.usage_tracker.cumulative_usage().output_tokens;
        self.session
            .push_user_text(user_input)
            .map_err(|error| RuntimeError::new(error.to_string()))?;

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
        self.begin_step_effort_turn(is_continuation);
        self.progress_marker_at_last_signal = 0;
        self.full_compactions_this_turn = 0;
        // Fold refusal history only at a PUBLIC boundary. Continuations are
        // additional legs of the same user turn and must not double-count it.
        if !is_continuation {
            self.fold_finished_refusal_turn();
        }
        // Clear the per-leg refusal override before deciding whether the
        // session cooldown should re-arm it below.
        self.refusal_fallback_model = None;
        self.refusal_same_model_retry_used = false;
        // Reset the per-turn quota fallback, pre-arming onto it when the session
        // is still inside a recorded quota-dry cooldown. See
        // [`Self::begin_turn_quota_fallback`].
        self.begin_turn_quota_fallback();
        // Escalation freshness (sync mirror of the streaming turn entry):
        // followup continuations run inside the escalated instruction, so
        // they are exempt like internal subturns. See
        // [`Self::begin_turn_escalation`].
        self.begin_turn_escalation(is_continuation);
        // Refusal-dry sees the would-be wire model only after the ordinary
        // refusal reset and quota/escalation state have settled.
        self.begin_turn_refusal_fallback();
        // A fresh user turn is genuine new intent: reset the cross-turn re-read
        // tally so legitimately re-reading the same file across separate
        // user-driven turns never accumulates into a false cross-turn stop. An
        // auto-continuation leg — a `TurnEnd`/Stop-hook `followupMessage` that
        // `run_turn` re-injects (loop_count > 0), the sync analogue of a
        // streaming internal subturn — is NOT new user intent, so it KEEPS the
        // tally: a re-read loop that spans followup continuations must still trip
        // the cross-turn guard even though each leg resets the per-turn one.
        if !is_continuation {
            self.cross_turn_tool_fingerprints.clear();
            self.cross_turn_tool_repetition_pending_hard_stop_fps.clear();
            self.cross_turn_tool_repetition_hard_stop_fps.clear();
        }
        // NOTE: `consecutive_microcompacts` is intentionally NOT reset here — it
        // is a cross-turn thrash signal (per its field doc) that must survive
        // turn boundaries so a re-read loop spanning turns can still trip the
        // microcompact thrash-escape.
        Ok(turn_start_output_tokens)
    }

    /// 8c hard guarantee: a schema phase forces a captured `StructuredOutput`
    /// call. If the agent stopped without calling the configured tool, run one
    /// final turn with `tool_choice = Tool { name }`; the model must then emit
    /// that tool call, and we record its message so `final_structured_output`
    /// reads the captured input. Best-effort — a forced-turn error leaves the
    /// turn's result intact (the engine falls back to text extraction). A
    /// compliant agent already called the tool, so this fires no extra request.
    ///
    /// Appends any forced message to `assistant_messages` so the caller's
    /// `TurnSummary` includes it.
    fn force_structured_output_call(&mut self, assistant_messages: &mut Vec<ConversationMessage>) {
        let Some(tool_name) = self.structured_output_tool.clone() else {
            return;
        };
        let already_called = assistant_messages.iter().any(|message| {
            message.blocks.iter().any(|block| {
                matches!(block, ContentBlock::ToolUse { name, .. } if name == &tool_name)
            })
        });
        if already_called {
            return;
        }
        // Claude Fable 5.1 / Mythos 5.1 return 400 on a forced `tool_choice`
        // (`tool` and `any`). There the requirement rides the transcript — a
        // harness system message, append-only so the thinking blocks behind
        // it stay bound to their prefix — and the call is left to `auto`; the
        // provider's wire chokepoint downgrades any forced choice that still
        // reaches such a model, so the request cannot 400 either way.
        let tool_choice = if self
            .effective_request_model()
            .is_some_and(::api::rejects_forced_tool_choice)
        {
            self.absorb_wire_reminders_into_session(&[format!(
                "The `{tool_name}` tool call is required for this turn: open your response \
                 with that call, carrying the final result."
            )]);
            ::api::ToolChoice::Auto
        } else {
            ::api::ToolChoice::Tool { name: tool_name }
        };
        let Ok(request) = self.build_request(Some(tool_choice)) else {
            return;
        };
        if let Ok(events) = self.api_client.stream(request) {
            if let AssistantTurn::Content { message, usage, .. } =
                build_assistant_message(normalize_empty_assistant_stream(events))
            {
                if let Some(usage) = usage {
                    self.usage_tracker.record(usage);
                }
                if self.session.push_message(message).is_ok() {
                    if let Some(msg) = self.session.messages.last().cloned() {
                        assistant_messages.push(msg);
                    }
                }
            }
        }
    }

    /// One agentic turn: push `user_input`, drive the model/tool loop to a stop,
    /// then fire the `TurnEnd` hook and return its `followupMessage` (if any)
    /// alongside the summary so [`Self::run_turn`] can decide whether to
    /// continue the Stop-loop.
    // the agentic turn loop (model/tool iteration + Stop hook); request build
    // and telemetry already extracted to `turn_support`, the loop body is cohesive
    #[allow(clippy::too_many_lines)]
    fn run_turn_once(
        &mut self,
        user_input: String,
        prompter: &mut Option<&mut dyn PermissionPrompter>,
        loop_count: usize,
    ) -> Result<(TurnSummary, Option<String>), RuntimeError> {
        // Baseline for this turn's output-token delta (see `TurnSummary.turn_output_tokens`).
        // `loop_count > 0` marks a Stop-hook followup leg (auto-continuation): it
        // preserves the cross-turn re-read tally, mirroring the streaming
        // `internal_subturn` path.
        let turn_start_output_tokens = self.begin_turn_once(user_input, loop_count > 0)?;
        // Input-side baseline for the third cost breaker; captured here (after
        // `begin_turn_once`, which makes no provider call) so both token
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

        loop {
            if self.tool_loop_break_requested {
                // The repetition guard killed the loop. Report it like every
                // other non-convergent stop: reporting `None` here made the
                // host read a turn the harness had to kill as a clean one, so
                // `clear_turn_failure()` wiped the escalation ladder and grind
                // streak that this exact pattern should be feeding — and the
                // transcript was left ending on tool results with no closing
                // assistant message.
                let error = RuntimeError::new("turn hit the tool-repetition guard");
                self.record_turn_failed(iterations, &error);
                budget_exhausted = Some(BudgetExhausted::ToolRepetition);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::ToolRepetition,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(RuntimeError::new)?;
                eprintln!(
                    "[zo] {}",
                    budget_exhausted_notice(BudgetExhausted::ToolRepetition, iterations)
                );
                break;
            }
            iterations += 1;
            self.check_sync_turn_cancelled(iterations)?;
            if iterations > self.max_iterations {
                let error = RuntimeError::new(
                    "conversation loop exceeded the maximum number of iterations",
                );
                self.clear_empty_retry_reminder(empty_retries);
                // Budget exhausted, not failed: the loop only reaches this
                // boundary after a prior iteration closed the session
                // well-formed (assistant `tool_use` + user tool-results), so the
                // work so far is preserved instead of rolled back. Record the
                // failure signal for telemetry, append a synthetic closer so the
                // turn is well-formed and the cutoff is visible, and end the turn
                // Ok(..) with the budget marker — the caller (or user) can
                // continue in a follow-up. See [`BudgetExhausted`].
                self.record_turn_failed(iterations, &error);
                budget_exhausted = Some(BudgetExhausted::Iterations);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::Iterations,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(RuntimeError::new)?;
                eprintln!(
                    "[zo] {}",
                    budget_exhausted_notice(BudgetExhausted::Iterations, iterations)
                );
                break;
            }
            // The last iterations ask for the report; the final one forbids
            // tools, so a capped run ends on the model's answer (budget_wrap_up).
            let wrap_up_choice = self.arm_budget_wrap_up(iterations);
            // Wall-clock budget (spawned sub-agents only): a straggler that
            // overran its caller's wait window must stop, not keep running and
            // billing in the background. Checked between iterations — cooperative,
            // so the current provider stream finishes first. Same as the
            // iteration cap above: preserve the well-formed work and end Ok(..)
            // with the budget marker rather than rolling the turn back.
            if self
                .deadline
                .is_some_and(|d| std::time::Instant::now() >= d)
            {
                let error = RuntimeError::new("agent exceeded its time budget");
                self.clear_empty_retry_reminder(empty_retries);
                self.record_turn_failed(iterations, &error);
                budget_exhausted = Some(BudgetExhausted::Deadline);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::Deadline,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(RuntimeError::new)?;
                eprintln!(
                    "[zo] {}",
                    budget_exhausted_notice(BudgetExhausted::Deadline, iterations)
                );
                break;
            }
            // Output-token budget (cost circuit breaker): mirrors the streaming
            // loop. Bounds an agentic loop that keeps generating without
            // converging, the multi-day-runaway case the iteration cap misses.
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
                .map_err(RuntimeError::new)?;
                eprintln!(
                    "[zo] {}",
                    budget_exhausted_notice(BudgetExhausted::OutputTokens, iterations)
                );
                break;
            }
            // Input-token budget (cache-miss cost circuit breaker): mirrors the
            // streaming loop. Bounds a cache-dead loop that re-sends the whole
            // transcript uncached every call — millions of input tokens with
            // little output, invisible to the output breaker above.
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
                .map_err(RuntimeError::new)?;
                eprintln!(
                    "[zo] {}",
                    budget_exhausted_notice(BudgetExhausted::InputTokens, iterations)
                );
                break;
            }

            // Proactive compaction: compact before building the request. On
            // the first iteration use a local request estimate because provider
            // usage can still describe the previous turn; later iterations use
            // live usage plus the local estimate.
            if iterations == 1 {
                if let Some(event) = self.maybe_microcompact_preflight() {
                    microcompact.get_or_insert(event);
                }
                if let Some(event) = self.maybe_auto_compact_preflight() {
                    auto_compaction.get_or_insert(event);
                } else {
                    self.maybe_state_distill_preflight();
                }
            } else {
                if let Some(event) = self.maybe_microcompact() {
                    microcompact.get_or_insert(event);
                }
                if let Some(event) = self.maybe_auto_compact() {
                    auto_compaction.get_or_insert(event);
                } else {
                    self.maybe_state_distill();
                }
            }

            // A turn that pre-armed onto the quota fallback (session still
            // cooling down) has no render channel on this headless sync path, so
            // announce it on stderr — the headless notice convention (mirrors the
            // spec-literal gate's eprintln). Fires once per turn.
            if self.quota_prearm_notice_pending {
                self.quota_prearm_notice_pending = false;
                if let Some((_, model)) = self.quota_fallback_client.clone() {
                    eprintln!("[zo] {}", self.quota_prearm_notice_text(&model));
                }
            }
            if self.refusal_prearm_notice_pending {
                self.refusal_prearm_notice_pending = false;
                eprintln!("[zo] {REFUSAL_DRY_PREARM_WARN}");
            }

            let request = match self.build_request(wrap_up_choice) {
                Ok(request) => request,
                Err(error) => {
                    self.clear_empty_retry_reminder(empty_retries);
                    self.record_turn_failed(iterations, &error);
                    return Err(error);
                }
            };
            let events = match self.sync_stream_events(request) {
                Ok(events) => events,
                Err(error) => {
                    if !provider_overflow_recovery_attempted
                        && error.provider_error_class()
                            == Some(crate::ProviderErrorClass::ContextOverflow)
                    {
                        provider_overflow_recovery_attempted = true;
                        // Learn the wire's real ceiling before compacting, so this
                        // session's thresholds stop being derived from a window the
                        // provider does not honor.
                        if let Some(ceiling) =
                            ::api::context_overflow_ceiling_tokens(&error.to_string())
                        {
                            self.adopt_provider_context_ceiling(ceiling);
                        }
                        if let Some(event) = self.recover_provider_context_overflow() {
                            auto_compaction.get_or_insert(event);
                            continue;
                        }
                    }
                    // Main model quota exhausted: HOLD on the main model when its
                    // window lifts within the wait band, else swap this sync turn
                    // onto the cross-provider fallback (driven via a scoped runtime
                    // by `sync_stream_events`); re-request either way rather than
                    // dying. The headless path has no render channel, so notices
                    // go to stderr and the wait is a blocking sleep.
                    match self.decide_quota_escape(&error) {
                        QuotaEscape::Wait(wait) => {
                            let model = self.context_model.clone().unwrap_or_default();
                            eprintln!("[zo] {}", quota_wait_hold_warn(&model, wait));
                            std::thread::sleep(wait);
                            continue;
                        }
                        QuotaEscape::Fallback(model) => {
                            eprintln!("[zo] {}", quota_fallback_swap_warn(&model));
                            continue;
                        }
                        // Provider shed this tier: re-request one tier down on the
                        // same provider (the override is already recorded).
                        QuotaEscape::Lighter(model) => {
                            let shed = self
                                .context_model
                                .clone()
                                .unwrap_or_else(|| "the main model".to_string());
                            eprintln!("[zo] {}", overload_demotion_warn(&shed, &model));
                            continue;
                        }
                        QuotaEscape::None => {}
                    }
                    self.clear_empty_retry_reminder(empty_retries);
                    self.record_turn_failed(iterations, &error);
                    return Err(error);
                }
            };
            self.check_sync_turn_cancelled(iterations)?;
            let assistant_turn = build_assistant_message(normalize_empty_assistant_stream(events));
            // Anthropic safety-classifier refusal (`stop_reason: "refusal"`):
            // drop the refused partial (never pushed) and either retry once on
            // the Opus family head (Fable/Mythos) or surface a notice (already fell back, or
            // a non-Fable model). Anthropic-only — a non-Anthropic model yields
            // `Proceed` and falls through unchanged. See `decide_refusal_fallback`.
            if is_refusal_stop_reason(assistant_turn.stop_reason().unwrap_or_default()) {
                let refused_usage = assistant_turn.usage();
                match self.decide_refusal_fallback() {
                    RefusalDecision::Retry | RefusalDecision::RetrySameModel => {
                        if let Some(usage) = refused_usage {
                            self.usage_tracker.record(usage);
                        }
                        continue;
                    }
                    RefusalDecision::Surface => {
                        if let Some(usage) = refused_usage {
                            self.usage_tracker.record(usage);
                        }
                        let assistant_message = refusal_surfaced_message();
                        self.record_assistant_iteration(iterations, &assistant_message, 0);
                        self.session
                            .push_message(assistant_message)
                            .map_err(|error| RuntimeError::new(error.to_string()))?;
                        if let Some(msg) = self.session.messages.last().cloned() {
                            assistant_messages.push(msg);
                        }
                        break;
                    }
                    RefusalDecision::Proceed => {}
                }
            }
            let (assistant_message, usage, turn_prompt_cache_events, stop_reason) =
                match assistant_turn {
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
                    // Clean stop, no content (thinking-only / transient empty):
                    // record telemetry and re-request a bounded number of times
                    // before surfacing the original error, rather than throwing
                    // away the turn's work on a one-off empty completion.
                    AssistantTurn::Empty { usage, stop_reason } => {
                        match self.handle_empty_assistant_turn(
                            usage,
                            stop_reason.as_deref(),
                            &mut empty_retries,
                            &mut empty_recovery_attempted,
                        ) {
                            EmptyAssistantAction::Retry | EmptyAssistantAction::ContinueOnce => {
                                continue;
                            }
                            EmptyAssistantAction::Exhausted => {}
                        }
                        let assistant_message = empty_stream_exhausted_message();
                        self.record_assistant_iteration(iterations, &assistant_message, 0);
                        self.session
                            .push_message(assistant_message)
                            .map_err(|error| RuntimeError::new(error.to_string()))?;
                        if let Some(msg) = self.session.messages.last().cloned() {
                            assistant_messages.push(msg);
                        }
                        break;
                    }
                };
            if let Some(usage) = usage {
                self.usage_tracker.record(usage);
            }
            self.record_prompt_cache_breaks(iterations, &turn_prompt_cache_events);
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
                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                if let Some(msg) = self.session.messages.last().cloned() {
                    assistant_messages.push(msg);
                }
                // Text-only turn boundary — mirrors the streaming loop's seam
                // (A): the tool-result drain below is never reached on a turn
                // the model answers with prose alone, so steering delivered
                // mid-turn (a `SendMessage` to this running sub-agent, or
                // type-ahead on a sync host) would otherwise strand in the
                // queue. Fold pending steers — and/or a truncation
                // continuation, when the provider cut the response off at the
                // output-token limit — into a fresh user turn and run one more
                // iteration; the last message is now this assistant reply, so
                // a new user turn is well-formed. Truncation continuations are
                // bounded so a model that keeps over-spending the window can't
                // loop forever.
                let steers = self.drain_steering();
                let truncated = take_truncation_continuation(
                    stop_reason.as_deref(),
                    &mut truncation_continuations,
                );
                if steers.is_empty() && !truncated {
                    // Turn-end gate, sync-loop mirror of the streaming seam.
                    // An autonomous surface (a headless one-shot) takes the
                    // whole gate; a sub-agent shares this loop but carries its
                    // own completion contract, so it takes only the
                    // malformed-reply half — a tool call written as text is
                    // not a reply anywhere, and a helper that ends on one
                    // hands its parent nothing.
                    let issue = if self.autonomous_surface {
                        self.take_turn_end_gate_issue(
                            &final_visible_text,
                            &mut turn_end_gate_reprompts,
                        )
                    } else {
                        turn_end_gate::take_textual_tool_call_issue(
                            &final_visible_text,
                            &mut turn_end_gate_reprompts,
                        )
                    };
                    if let Some(issue) = issue {
                        eprintln!("[zo] {}", turn_end_gate::turn_end_gate_banner(issue));
                        self.session
                            .push_user_text(self.turn_end_gate_reminder_text(issue))
                            .map_err(|error| RuntimeError::new(error.to_string()))?;
                        continue;
                    }
                    if let Some(receipt) = self.completion_receipt_line() {
                        eprintln!("[zo] {receipt}");
                    }
                    break;
                }
                let mut continuation_text = String::new();
                if truncated {
                    continuation_text.push_str(TRUNCATION_CONTINUATION_REMINDER);
                }
                for steer in steers {
                    if !continuation_text.is_empty() {
                        continuation_text.push_str("\n\n");
                    }
                    continuation_text.push_str(&steering_message(&steer));
                }
                self.session
                    .push_user_text(continuation_text)
                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                continue;
            }
            if let Err(error) = self.check_tool_call_budget(tool_calls, pending_tool_use_count) {
                self.record_turn_failed(iterations, &error);
                // Tool-call budget: the over-budget assistant `tool_use` batch is
                // not yet in the session, so the session still ends on the prior
                // well-formed `user` message. Drop the pending batch (it would be
                // orphaned — no results will be produced), append a closer, and
                // end Ok(..) with the marker so the work already done survives.
                budget_exhausted = Some(BudgetExhausted::ToolCalls);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::ToolCalls,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(RuntimeError::new)?;
                eprintln!(
                    "[zo] {}",
                    budget_exhausted_notice(BudgetExhausted::ToolCalls, iterations)
                );
                break;
            }
            tool_calls += pending_tool_use_count;
            self.check_sync_turn_cancelled(iterations)?;

            let tool_uses = collect_pending_tool_uses(&assistant_message);

            self.session
                .push_message(assistant_message)
                .map_err(|error| RuntimeError::new(error.to_string()))?;
            if let Some(msg) = self.session.messages.last().cloned() {
                assistant_messages.push(msg);
            }

            let mut prepared = Vec::with_capacity(tool_uses.len());
            for tool_use in &tool_uses {
                let tool_use_id = &tool_use.id;
                let tool_name = &tool_use.name;
                let input = &tool_use.input;
                let __hook_t = std::time::Instant::now();
                let pre_hook_result = self.run_pre_tool_use_hook(tool_name, input);
                if __hook_t.elapsed().as_millis() >= 50 && crate::turn_profiling_enabled() {
                    eprintln!(
                        "[TURN-SEG] pre_tool_hook({tool_name}) = {}ms (synchronous; starves render_tick)",
                        __hook_t.elapsed().as_millis()
                    );
                }
                let effective_input = pre_hook_result.updated_input().map_or_else(
                    || Cow::Borrowed(input.as_str()),
                    |updated| Cow::Owned(updated.to_owned()),
                );
                let permission_context = PermissionContext::new(
                    pre_hook_result.permission_override(),
                    pre_hook_result.permission_reason().map(ToOwned::to_owned),
                );

                let permission_outcome = if let Some(outcome) =
                    pre_hook_denial_outcome(&pre_hook_result, tool_name)
                {
                    outcome
                } else if let Some(reason) = self.architect_edit_gate_denial(tool_name) {
                    // Architect contract backstop — sync mirror of the
                    // streaming path's gate, so headless/sub-agent hosts that
                    // arm the gate get identical enforcement.
                    PermissionOutcome::Deny { reason }
                } else if let Some(prompt) = prompter.as_mut() {
                    self.permission_policy.authorize_with_context(
                        tool_name,
                        effective_input.as_ref(),
                        &permission_context,
                        Some(*prompt),
                    )
                } else {
                    self.permission_policy.authorize_with_context(
                        tool_name,
                        effective_input.as_ref(),
                        &permission_context,
                        None,
                    )
                };

                if let PermissionOutcome::Deny { reason } = &permission_outcome {
                    self.fire_lifecycle_hook(
                        HookEvent::PermissionDenied,
                        &serde_json::json!({ "tool_name": tool_name, "reason": reason }),
                    );
                }
                prepared.push(PreparedSyncTool {
                    tool_use_id: tool_use_id.clone(),
                    tool_name: tool_name.clone(),
                    effective_input,
                    pre_hook_result,
                    permission_outcome,
                });
            }

            let mut precomputed: HashMap<usize, (String, bool)> = HashMap::new();
            // The waves: consecutive reads and spawns run side by side when the
            // serial pass below reaches them (`parallel_waves`). This sync path
            // spawns a raw OS thread per tool, so each wave is capped at
            // MAX_PARALLEL_SAFE_TOOL_DISPATCHES: nine reads run 8 + 1, never
            // more than the cap of threads at once (BUG-D8-a).
            let waves = if self.sync_parallel_batch_has_repetition_risk(&prepared) {
                Vec::new()
            } else {
                parallel_waves(
                    prepared
                        .iter()
                        .enumerate()
                        .map(|(idx, p)| (idx, p.tool_name.as_str(), &p.permission_outcome)),
                )
            };
            let mut waves = waves.into_iter().peekable();
            // Where THIS batch's results begin, so the treadmill can ask what the
            // batch actually accomplished rather than what it merely attempted.
            let batch_results_from = tool_results.len();
            let mut batch_hard_stops = ToolBatchRepetitionHardStops::default();
            for (idx, p) in prepared.iter().enumerate() {
                if waves.peek().is_some_and(|wave| wave[0] == idx) {
                    let wave = waves.next().unwrap_or_default();
                    if let Some(dispatch) = &self.concurrent_dispatch {
                        for batch in wave.chunks(MAX_PARALLEL_SAFE_TOOL_DISPATCHES) {
                            let mut handles = Vec::with_capacity(batch.len());
                            for &idx in batch {
                                let p = &prepared[idx];
                                let dispatch = Arc::clone(dispatch);
                                let name = p.tool_name.clone();
                                let input = p.effective_input.to_string();
                                handles.push((
                                    idx,
                                    std::thread::spawn(move || match dispatch(&name, &input) {
                                        Ok(output) => (output, false),
                                        Err(e) => (e.to_string(), true),
                                    }),
                                ));
                            }
                            for (idx, handle) in handles {
                                let result = handle.join().unwrap_or_else(|_| {
                                    ("parallel tool execution panicked".to_string(), true)
                                });
                                precomputed.insert(idx, result);
                            }
                        }
                    }
                }
                let result_message = match &p.permission_outcome {
                    PermissionOutcome::Allow => {
                        let synthetic_output = batch_hard_stops.preflight_notice(
                            &p.tool_name,
                            p.effective_input.as_ref(),
                            || {
                                self.next_tool_repetition_hard_stop_notice(
                                    &p.tool_name,
                                    p.effective_input.as_ref(),
                                )
                            },
                        );
                        if let Some((output, terminates)) = synthetic_output {
                            if terminates {
                                self.tool_loop_break_requested = true;
                            }
                            tool_result_message(
                                p.tool_use_id.as_str(),
                                p.tool_name.as_str(),
                                merge_hook_context(
                                    p.pre_hook_result.additional_context_messages(),
                                    merge_hook_feedback(p.pre_hook_result.messages(), output, true),
                                ),
                                true,
                                Vec::new(),
                            )
                        } else {
                            self.record_tool_started(iterations, &p.tool_name);
                        let tool_start = std::time::Instant::now();
                        let (mut output, mut is_error) =
                            if let Some(result) = precomputed.remove(&idx) {
                                result
                            } else {
                                match self
                                    .tool_executor
                                    .execute(&p.tool_name, p.effective_input.as_ref())
                                {
                                    Ok(output) => (output, false),
                                    Err(error) => (error.to_string(), true),
                                }
                            };
                        crate::notifications::notify_if_slow(
                            &p.tool_name,
                            tool_start,
                            std::time::Duration::from_secs(10),
                        );
                        // Kept before any hook merges text into it: the patch
                        // review seat reads the tool's own envelope (t-6203).
                        let reviewed = (!is_error && self.reviews_edits_of(&p.tool_name))
                            .then(|| output.clone());
                        output = merge_hook_feedback(p.pre_hook_result.messages(), output, false);

                        let post_hook_result = if is_error {
                            self.run_post_tool_use_failure_hook(
                                &p.tool_name,
                                p.effective_input.as_ref(),
                                &output,
                            )
                        } else {
                            self.run_post_tool_use_hook(
                                &p.tool_name,
                                p.effective_input.as_ref(),
                                &output,
                                false,
                            )
                        };
                        if post_hook_result.is_denied()
                            || post_hook_result.is_failed()
                            || post_hook_result.is_cancelled()
                        {
                            is_error = true;
                        }
                        output = merge_hook_feedback(
                            post_hook_result.messages(),
                            output,
                            post_hook_result.is_denied()
                                || post_hook_result.is_failed()
                                || post_hook_result.is_cancelled(),
                        );
                        // Prompt material rides the model-facing result only —
                        // the streaming twin explains why it is a separate seam.
                        output = merge_hook_context(
                            p.pre_hook_result.additional_context_messages(),
                            output,
                        );
                        output = merge_hook_context(
                            post_hook_result.additional_context_messages(),
                            output,
                        );
                        // Enforcer-layer denials surface as tool errors here;
                        // fold same-class repeats like the policy-layer arm.
                        if is_error {
                            output = self.fold_repeated_mode_denial(&p.tool_name, output);
                        }
                        self.append_tool_repetition_notice(
                            &mut output,
                            &p.tool_name,
                            p.effective_input.as_ref(),
                            is_error,
                            &mut batch_hard_stops,
                        );
                        if let Some(pristine) = reviewed.filter(|_| !is_error) {
                            let note = self.reviewed_edit_note_blocking(
                                &p.tool_use_id,
                                &p.tool_name,
                                &pristine,
                            );
                            output = reviewed_edit::with_review_note(output, note);
                        }

                        // Drain any images the tool staged (single-threaded: image
                        // tools run on this serial path). Drained unconditionally so
                        // a stale image can never leak onto the next tool result.
                        let images = self.tool_executor.take_pending_images();
                        tool_result_message(
                            p.tool_use_id.as_str(),
                            p.tool_name.as_str(),
                            output,
                            is_error,
                            images,
                        )
                        }
                    }
                    PermissionOutcome::Deny { reason } => {
                        let body =
                            self.fold_repeated_mode_denial(&p.tool_name, denial_result_body(reason));
                        ConversationMessage::tool_result(
                            p.tool_use_id.as_str(),
                            p.tool_name.as_str(),
                            merge_hook_context(
                                p.pre_hook_result.additional_context_messages(),
                                merge_hook_feedback(p.pre_hook_result.messages(), body, true),
                            ),
                            true,
                        )
                    }
                };
                self.session
                    .push_message(result_message.clone())
                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                self.record_tool_finished(iterations, &result_message, p.effective_input.as_ref());
                self.note_progress_tool_result(&result_message);
                tool_results.push(result_message);
            }
            self.arm_tool_repetition_hard_stops();
            // Verification-treadmill circuit breaker: a batch that plans/validates/
            // spawns (verify-class) but changes no file is a self-verification round.
            // Too many in a row without progress stop the turn gracefully — the case
            // the repetition guard misses because each round carries a new spec, so
            // the identical-call fingerprint never matches. Same preserve+Ok closer
            // as the other budgets.
            let had_verify = tool_uses
                .iter()
                .any(|tool_use| is_verify_class_tool(&tool_use.name));
            let had_mutation = batch_had_successful_mutation(&tool_results[batch_results_from..]);
            if self.note_verify_treadmill(had_verify, had_mutation) == TurnContinuity::Stop {
                let error = RuntimeError::new("turn hit the verification treadmill");
                self.record_turn_failed(iterations, &error);
                budget_exhausted = Some(BudgetExhausted::VerificationTreadmill);
                self.push_budget_exhausted_closer(
                    BudgetExhausted::VerificationTreadmill,
                    iterations,
                    &mut assistant_messages,
                )
                .map_err(RuntimeError::new)?;
                eprintln!(
                    "[zo] {}",
                    budget_exhausted_notice(BudgetExhausted::VerificationTreadmill, iterations)
                );
                break;
            }
            // Mid-turn steering boundary — mirrors the streaming loop: fold
            // any steering delivered during this tool batch (a `SendMessage`
            // to this running sub-agent, or type-ahead on a sync host) into
            // the LAST tool-result message as extra Text blocks. Tool-result
            // messages serialize as wire role "user", so a separate user
            // message here would be two consecutive "user" turns (which the
            // API rejects); appending keeps one valid turn.
            let steers = self.drain_steering();
            if !steers.is_empty() {
                let messages = Arc::make_mut(&mut self.session.messages);
                if let Some(last) = messages.last_mut() {
                    for steer in steers {
                        last.blocks.push(ContentBlock::Text {
                            text: steering_message(&steer),
                        });
                    }
                    self.session.mark_transcript_dirty();
                }
            }
            // Mid-turn agent-notification boundary — mirrors the streaming
            // loop: fold background-agent completions the host staged during
            // this tool batch into the same last tool-result message, so the
            // model learns of finished agents without ending its turn.
            let notifications = self.drain_agent_notifications();
            if !notifications.is_empty() {
                let messages = Arc::make_mut(&mut self.session.messages);
                if let Some(last) = messages.last_mut() {
                    for notification in notifications {
                        last.blocks.push(ContentBlock::Text {
                            text: agent_notification_text(&notification),
                        });
                    }
                    self.session.mark_transcript_dirty();
                }
            }
            // Re-anchor the live plan after this tool batch so the next model
            // request keeps the in-progress todo item in view across the turn.
            self.nudge_serial_reads_after_batch();
            self.govern_step_after_batch();
            self.reinject_todo_progress_reminder();
            self.fire_post_batch_lifecycle_hooks();
        }

        // 8c hard guarantee: a schema phase forces a captured `StructuredOutput`
        // call. See [`Self::force_structured_output_call`]. Skipped on a
        // budget-exhausted turn: it would spend another provider round-trip past
        // the budget and append a second consecutive assistant message right
        // after the closer, breaking user/assistant alternation.
        if budget_exhausted.is_none() {
            self.force_structured_output_call(&mut assistant_messages);
        }

        if let Some(event) = self.maybe_microcompact() {
            microcompact.get_or_insert(event);
        }
        if let Some(event) = self.maybe_auto_compact() {
            auto_compaction.get_or_insert(event);
        } else {
            self.maybe_state_distill();
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

        // Skip the per-turn `git diff` subprocess (the dominant turn-end stall on
        // a dirty repo) when no `TurnEnd` hook is configured: with zero matching
        // commands `run_lifecycle_hook` is a no-op that consumes none of the
        // context, so computing `files_changed` would be pure waste. Only pay for
        // the snapshot when a hook will actually read it.
        let has_turn_end_hook = self.hook_runner.lifecycle_command_count(HookEvent::TurnEnd) > 0;
        let files_changed = if has_turn_end_hook {
            changed_files_snapshot()
        } else {
            Vec::new()
        };
        let turn_end_context = build_turn_end_hook_context(
            &summary,
            loop_count,
            &files_changed,
            self.session.session_goal.as_deref(),
        );
        let turn_end = self.run_lifecycle_hook(HookEvent::TurnEnd, &turn_end_context);

        Ok((summary, turn_end.followup().map(str::to_owned)))
    }

}


#[cfg(test)]
mod tests;
