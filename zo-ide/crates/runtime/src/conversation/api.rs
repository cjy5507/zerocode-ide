//! Provider-side contracts for the conversation loop.
//!
//! Two traits and three value types make up the seam between the runtime
//! and any upstream model provider (Anthropic, OpenAI-compat, mock, …):
//!
//! - [`ApiClient`] — synchronous streaming contract used by the legacy
//!   `run_turn` path.
//! - [`AsyncApiClient`] — async/streaming contract used by
//!   [`ConversationRuntime::run_turn_streaming`]; emits live
//!   [`RenderBlock`] deltas while still producing the accumulated event
//!   list needed by `build_assistant_message`.
//! - [`ApiRequest`] / [`AssistantEvent`] / [`PromptCacheEvent`] —
//!   provider-neutral value types passed across both traits.

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::message_stream::types::RenderBlock;
use crate::session::ConversationMessage;
use crate::usage::TokenUsage;

use super::RuntimeError;

/// Default bounded capacity for the streaming `RenderBlock` channel.
///
/// Chosen to balance backpressure honesty (code-rules R8 — bounded only)
/// with the observation that a single turn typically emits a few dozen
/// blocks (text deltas + 1–3 tool call pairs). 64 leaves headroom for a
/// well-behaved TUI consumer while still stalling a slow/hung one within
/// a few iterations. L7c may revisit after real TUI profiling.
pub const DEFAULT_STREAMING_CHANNEL_CAPACITY: usize = 64;

/// Fully assembled request payload sent to the upstream model client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    pub system_prompt: Arc<[String]>,
    /// Wire-only harness reminders the lowering seam appends to the newest
    /// user-role wire message via [`crate::append_wire_reminders`]. **Empty on
    /// the runtime path**: the conversation runtime persists its per-turn
    /// reminders (recalled memory, todo progress, …) into `messages` as a
    /// trailing `System` transcript message instead
    /// (`crate::ConversationRuntime::absorb_wire_reminders_into_session`) —
    /// a wire-only reminder moves with the tail, so the previous request's
    /// newest message re-rendered without it and every request re-billed the
    /// prior tail as uncached input. Kept out of `system_prompt` for the same
    /// cache reason (`system_changed` invalidates every message breakpoint
    /// behind it). The field remains for hand-assembled requests.
    pub wire_reminders: Arc<[String]>,
    pub messages: Arc<Vec<ConversationMessage>>,
    /// Forced tool selection for this request. `None` (the default) lets the
    /// client choose (`auto`); the runtime sets `Tool { name }` only for the
    /// final structured-output turn (workflow 8c), so a schema phase is
    /// guaranteed to emit the captured tool call.
    pub tool_choice: Option<::api::ToolChoice>,
    /// Per-request reasoning-effort **floor** as a thinking budget (tokens),
    /// or `None` for the client's configured default. Treated as a floor, not
    /// an override: the client uses `max(this, its own budget)`, so it can only
    /// *raise* effort, never lower it. The deep-gate sets this on a stalled
    /// retry (`auto_effort_for_prompt` starts low — `Off` for fixes — and the
    /// gate's doc delegates "step up to Xhigh on a stalled retry" to runtime
    /// escalation; this is that seam). `None` on every ordinary turn, so the
    /// configured effort is unchanged unless escalation explicitly engages.
    pub effort_override: Option<u32>,
    /// The step effort governor's effort for THIS request
    /// (`conversation/step_effort.rs`), or `None` for the client's own. When
    /// set, a client puts it on the wire request as `effort` and
    /// `effort_band_ceiling` in place of the turn's — the same two fields
    /// every backend already reads — and sizes any legacy thinking budget
    /// from the same level. Unlike `effort_override` it may LOWER effort: a
    /// routine step spends less on purpose.
    pub effort_step: Option<super::EffortStep>,
    /// Per-turn wire-model override, or `None` for the client's bound model.
    /// The runtime sets it VERBATIM when an Anthropic safety classifier declines
    /// a Fable/Mythos turn (`stop_reason: "refusal"`): the turn is retried once
    /// on the provider catalog's current Opus family head, following
    /// Anthropic's client-side fallback guidance.
    /// Clients that build a wire `MessageRequest` honor it verbatim (only the
    /// wire model id and its `max_tokens` change; the bound provider client is
    /// unchanged — the fallback target is Anthropic, same as the refused model).
    /// It is Anthropic-only in practice — a refusal never arises on a
    /// non-Anthropic provider — and never persists past the current turn.
    pub model_override: Option<String>,
}

pub const GEMINI_THOUGHT_SIGNATURE_PROVIDER_STATE: &str = "gemini.thought_signature";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStateBlob {
    provider: String,
    kind: String,
    value: String,
}

impl ProviderStateBlob {
    #[must_use]
    pub fn new(
        provider: impl Into<String>,
        kind: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            kind: kind.into(),
            value: value.into(),
        }
    }

    #[must_use]
    pub fn gemini_thought_signature(signature: impl Into<String>) -> Self {
        Self::new("google", GEMINI_THOUGHT_SIGNATURE_PROVIDER_STATE, signature)
    }

    #[must_use]
    pub fn as_gemini_thought_signature(&self) -> Option<&str> {
        let provider = self.provider.as_str();
        let is_google_provider = provider.eq_ignore_ascii_case("google")
            || provider.eq_ignore_ascii_case("gemini")
            || provider.eq_ignore_ascii_case("gemini_code_assist");
        (is_google_provider && self.kind == GEMINI_THOUGHT_SIGNATURE_PROVIDER_STATE)
            .then_some(self.value.as_str())
    }
}

/// Streamed events emitted while processing a single assistant turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistantEvent {
    TextDelta(String),
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    Usage(TokenUsage),
    PromptCache(PromptCacheEvent),
    /// Provider-declared stop reason for this turn, e.g. `"end_turn"`,
    /// `"tool_use"`, or a truncation marker (`"max_tokens"` for Anthropic,
    /// `"length"` for OpenAI). Emitted at most once per turn when the provider
    /// reports one; its position relative to [`AssistantEvent::MessageStop`] is
    /// not significant — the conversation loop scans the whole event list. It
    /// reads this to distinguish a natural turn end from a turn cut short at the
    /// output-token limit (see `is_truncation_stop_reason`), so a truncated turn
    /// can be continued instead of mistaken for completion.
    StopReason(String),
    /// Provider-opaque reasoning signature for this turn (Gemini 3's
    /// `thoughtSignature`), surfaced from the response so the assistant message
    /// carries it for same-provider round-tripping. Emitted at most once per
    /// turn; ignored by providers that don't mint one.
    ThoughtSignature(String),
    /// A completed Anthropic reasoning block (extended / interleaved thinking)
    /// from this turn, captured in arrival order so it is stored and replayed
    /// verbatim on the next Anthropic request. `signature` is `None` when the
    /// provider streamed no signature (e.g. `display:"omitted"` with no
    /// signature, or older data); such a block is stored but dropped at replay
    /// — an unsigned thinking block 400s. Ignored by non-Anthropic providers.
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    /// A completed Anthropic `redacted_thinking` block (encrypted reasoning),
    /// captured for verbatim replay on the Anthropic path.
    RedactedThinking {
        data: String,
    },
    ProviderState(ProviderStateBlob),
    /// ChatGPT/Codex reasoning-replay payload for this turn — the Responses
    /// reasoning items that must be echoed back before each `function_call`
    /// to keep multi-turn reasoning continuity (see
    /// [`core_types::ConversationMessage::reasoning_replay`] and
    /// `api::providers::chatgpt_backend`). Emitted at most once per turn;
    /// ignored by providers that don't emit one.
    ReasoningReplay(serde_json::Value),
    /// Wire model id the provider reported for this response (`message.model`
    /// on `message_start`, or the non-stream response's `model`). Stamped
    /// onto the stored assistant message for per-model cost attribution —
    /// smart routing interleaves models turn-by-turn and fallbacks can swap
    /// mid-turn, so the session's own notion of "the model" cannot say which
    /// model billed which usage record. Emitted at most once per turn; absent
    /// when the provider reports no model id.
    Model(String),
    /// The category a provider's safety classifier named for this turn's
    /// refusal — `stop_details.category` beside a `stop_reason: "refusal"`
    /// (t-6747). Emitted at most once per turn, only when the refusal names
    /// one: the refusal ladder routes by it, and a category routed nowhere
    /// stands. See [`push_refusal_category`].
    RefusalCategory(String),
    MessageStop,
}

/// Push the refusal category a stop's details name, beside the stop reason —
/// the one spelling every provider path emits it in (t-6747). A blank or
/// absent category pushes nothing: the refusal names none.
pub fn push_refusal_category(events: &mut Vec<AssistantEvent>, category: Option<&str>) {
    if let Some(category) = category.map(str::trim).filter(|word| !word.is_empty()) {
        events.push(AssistantEvent::RefusalCategory(category.to_string()));
    }
}

/// The refusal category a turn's events carry, when they carry one.
#[must_use]
pub fn refusal_category_of(events: &[AssistantEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        AssistantEvent::RefusalCategory(category) => Some(category.clone()),
        _ => None,
    })
}

/// Lower a streamed `redacted_thinking` block's `data` (an opaque JSON value —
/// in practice a base64 string) to the `String` the session stores and the
/// Anthropic wire expects. A JSON string is taken as-is; anything else is
/// stringified so nothing is lost across the round-trip.
#[must_use]
pub fn redacted_thinking_data_to_string(data: &serde_json::Value) -> String {
    match data {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Append the runtime events for a single completed output block, stashing a
/// `tool_use` into `pending_tools` keyed by `block_index` — its accumulated
/// arguments are flushed as [`AssistantEvent::ToolUse`] by the caller once the
/// block closes (so parallel tool calls keyed by distinct indices never splice
/// their arguments).
///
/// `streaming_tool_input` marks the streaming-start path, where a `tool_use`
/// block ships an empty-object placeholder whose real arguments arrive later as
/// `input_json_delta`; in that case the buffered input starts empty rather than
/// as the literal `{}`. This is the headless, render-free transform shared by
/// the sub-agent provider client and the non-streaming fallback below; the TUI
/// path keeps its own `out`-writing variant.
pub fn push_output_block(
    block: ::api::OutputContentBlock,
    block_index: u32,
    events: &mut Vec<AssistantEvent>,
    pending_tools: &mut std::collections::BTreeMap<u32, (String, String, String)>,
    streaming_tool_input: bool,
) {
    match block {
        ::api::OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        ::api::OutputContentBlock::ToolUse { id, name, input } => {
            let initial_input = if streaming_tool_input
                && input.is_object()
                && input.as_object().is_some_and(serde_json::Map::is_empty)
            {
                String::new()
            } else {
                input.to_string()
            };
            pending_tools.insert(block_index, (id, name, initial_input));
        }
        ::api::OutputContentBlock::Thinking { thinking, signature } => {
            // In the streaming paths the `content_block_start` ships an empty
            // placeholder; the real text and signature arrive as thinking /
            // signature deltas, which the streaming callers accumulate and flush
            // themselves. Capture here only on the non-streaming path (complete
            // blocks), mirroring the tool-input placeholder rule above.
            if !streaming_tool_input {
                events.push(AssistantEvent::Thinking { thinking, signature });
            }
        }
        ::api::OutputContentBlock::RedactedThinking { data } => {
            // Redacted blocks arrive complete on `content_block_start` (no
            // deltas), so capture them the same way on every path.
            events.push(AssistantEvent::RedactedThinking {
                data: redacted_thinking_data_to_string(&data),
            });
        }
    }
}

/// Lower a non-streaming `MessageResponse` into the accumulated
/// [`AssistantEvent`] sequence — the headless fallback used when a streamed
/// turn yields no `message_stop` (empty-stream recovery re-requests with
/// `stream:false`).
///
/// Event order is fixed and must stay stable: each block in order (every
/// `tool_use` flushed immediately after its block), then `Usage`, then the
/// Gemini `thought_signature` provider-state (only when present), then the
/// ChatGPT `reasoning_replay` payload (only when present), then the provider
/// `stop_reason` (only when present and non-empty, so the conversation loop's
/// truncation recovery engages on this path too), then `MessageStop`.
#[must_use]
pub fn response_to_events(response: ::api::MessageResponse) -> Vec<AssistantEvent> {
    let mut events = Vec::new();
    let mut pending_tools = std::collections::BTreeMap::new();

    if !response.model.is_empty() {
        events.push(AssistantEvent::Model(response.model.clone()));
    }

    for (index, block) in response.content.into_iter().enumerate() {
        let index = u32::try_from(index).expect("response block index overflow");
        push_output_block(block, index, &mut events, &mut pending_tools, false);
        if let Some((id, name, input)) = pending_tools.remove(&index) {
            events.push(AssistantEvent::ToolUse { id, name, input });
        }
    }

    events.push(AssistantEvent::Usage(response.usage.token_usage()));
    if let Some(signature) = &response.thought_signature {
        events.push(AssistantEvent::ProviderState(
            ProviderStateBlob::gemini_thought_signature(signature.clone()),
        ));
    }
    if let Some(replay) = &response.reasoning_replay {
        events.push(AssistantEvent::ReasoningReplay(replay.clone()));
    }
    if let Some(reason) = response
        .stop_reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
    {
        events.push(AssistantEvent::StopReason(reason.to_string()));
    }
    push_refusal_category(
        &mut events,
        response
            .stop_details
            .as_ref()
            .and_then(|details| details.category.as_deref()),
    );
    events.push(AssistantEvent::MessageStop);
    events
}

/// Prompt-cache telemetry captured from the provider response stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheEvent {
    pub unexpected: bool,
    pub reason: String,
    pub previous_cache_read_input_tokens: u32,
    pub current_cache_read_input_tokens: u32,
    pub token_drop: u32,
    /// One-line, one-time-per-streak cache-efficiency warning
    /// (`PromptCacheRecord::low_cache_hit_warning`) — e.g. "prompt cache
    /// degraded: 3 consecutive requests re-billed ~180k tokens (history
    /// diverges at message #12/241)". `None` on the overwhelming majority of
    /// events. Populated independently of `cache_break` above: a session
    /// whose cache *stays* cold (rather than freshly dropping) never trips
    /// `detect_cache_break` — no minimum-token *drop* ever occurs — so this is
    /// the only signal that surfaces that failure mode. The streaming turn
    /// loop renders it live as a `System { Warn }` line (`[cache] …`); it
    /// also reaches the headless JSON turn result
    /// (`prompt_cache_events[].warning`) for OTLP/HUD consumers.
    pub warning: Option<String>,
}

/// Convert api-level prompt-cache bookkeeping into the runtime event carried
/// through foreground and sub-agent streams. A record with neither a cache
/// break nor a low-cache-hit warning updates stats only and intentionally
/// emits no event.
#[must_use]
pub fn prompt_cache_record_to_event(record: api::PromptCacheRecord) -> Option<PromptCacheEvent> {
    let warning = record.low_cache_hit_warning;
    if record.cache_break.is_none() && warning.is_none() {
        return None;
    }
    let (unexpected, reason, previous_cache_read_input_tokens, current_cache_read_input_tokens, token_drop) =
        match record.cache_break {
            Some(cache_break) => (
                cache_break.unexpected,
                cache_break.reason,
                cache_break.previous_cache_read_input_tokens,
                cache_break.current_cache_read_input_tokens,
                cache_break.token_drop,
            ),
            // Warning-only event: no break occurred (the cache is staying
            // cold, not freshly dropping), so there is no before/after token
            // pair to report — zeroed rather than omitted so the struct shape
            // stays uniform for downstream consumers.
            None => (false, String::new(), 0, 0, 0),
        };
    Some(PromptCacheEvent {
        unexpected,
        reason,
        previous_cache_read_input_tokens,
        current_cache_read_input_tokens,
        token_drop,
        warning,
    })
}

/// Drain partially streamed tool-use blocks whose `content_block_stop` did not
/// arrive before a clean provider stop. Claude can legally finish with
/// `message_stop` after streaming all `input_json_delta` bytes; without this the
/// conversation contains a visible tool call but no executable `ToolUse` event.
pub fn flush_pending_tool_events(
    events: &mut Vec<AssistantEvent>,
    pending_tools: &mut BTreeMap<u32, (String, String, String)>,
) {
    for (_, (id, name, input)) in std::mem::take(pending_tools) {
        events.push(AssistantEvent::ToolUse { id, name, input });
    }
}

/// Record cache-token usage for non-Anthropic providers that expose provider-side
/// cache reads (for example OpenAI-compatible `cached_tokens`). Anthropic keeps
/// its own prompt-cache tracker inside the API client because it also owns
/// `cache_control` placement; this helper fills the stats/event gap for other
/// providers without duplicating wire-format logic.
pub fn record_non_anthropic_prompt_cache_usage(
    session_id: &str,
    provider: api::ProviderKind,
    request: &api::MessageRequest,
    events: &mut Vec<AssistantEvent>,
) {
    if provider == api::ProviderKind::Anthropic || !provider.supports_cache_tokens() {
        return;
    }
    let Some(usage) = events.iter().rev().find_map(|event| match event {
        AssistantEvent::Usage(usage) => Some(*usage),
        _ => None,
    }) else {
        return;
    };
    let record = api::PromptCache::new(session_id).record_usage(
        request,
        &api::Usage {
            input_tokens: usage.input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            output_tokens: usage.output_tokens,
            output_tokens_details: None,
        },
    );
    if let Some(event) = prompt_cache_record_to_event(record) {
        events.push(AssistantEvent::PromptCache(event));
    }
}

/// Minimal streaming API contract required by [`super::ConversationRuntime`].
pub trait ApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError>;
}

/// Asynchronous streaming API seam used by
/// [`super::ConversationRuntime::run_turn_streaming`] when a live provider
/// stack (`AnthropicStream` / SSE `EventSource`) is wired in.
///
/// The seam exists so that a single HTTP turn can produce **both**:
///
/// 1. Low-latency [`RenderBlock`] deltas pushed into `render_tx` while
///    the SSE response is still being consumed (the TUI feel), and
/// 2. The complete [`AssistantEvent`] sequence required by
///    `build_assistant_message` for the session bookkeeping path the
///    rest of `run_turn_streaming` relies on.
///
/// Implementations **must** drive the upstream stream to completion —
/// returning only when the message is finished or a transport/parse
/// error is encountered — and **must** emit every text delta they
/// observe through `render_tx` (tool-call/tool-result rendering remains
/// the runtime loop's responsibility, so that permission prompts keep
/// their round-trip ordering).
///
/// When the receiver on `render_tx` has been dropped, implementations
/// should short-circuit as soon as the next `send().await` fails and
/// propagate the channel-closed failure as a `RuntimeError` — the
/// runtime loop will translate it into
/// [`super::StreamingTurnError::Cancelled`].
///
/// The trait uses the hand-rolled `Pin<Box<dyn Future>>` pattern per
/// the L1 living standard (no `async-trait` crate).
pub trait AsyncApiClient: Send + Sync {
    /// Drive a single model turn, emitting render deltas into
    /// `render_tx` and returning the accumulated [`AssistantEvent`]
    /// sequence for downstream bookkeeping.
    ///
    /// `text_block_id` is the [`BlockId`](crate::message_stream::types::BlockId)
    /// the runtime has pre-allocated for the text channel of this
    /// iteration. Implementations should use it when emitting
    /// [`RenderBlock::TextDelta`] so block identity lines up with the
    /// rest of the iteration's rendering.
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: crate::message_stream::types::BlockId,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a,
        >,
    >;
}

#[cfg(test)]
mod helper_tests {
    use super::{
        flush_pending_tool_events, prompt_cache_record_to_event,
        record_non_anthropic_prompt_cache_usage, push_output_block, response_to_events,
        AssistantEvent,
    };
    use crate::usage::TokenUsage;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn usage(output_tokens: u32) -> ::api::Usage {
        ::api::Usage {
            input_tokens: 1,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            output_tokens,
            output_tokens_details: None,
        }
    }

    fn response(
        content: Vec<::api::OutputContentBlock>,
        stop_reason: Option<&str>,
        thought_signature: Option<&str>,
    ) -> ::api::MessageResponse {
        ::api::MessageResponse {
            id: "msg-test".to_string(),
            kind: "message".to_string(),
            role: "assistant".to_string(),
            content,
            model: "test-model".to_string(),
            stop_reason: stop_reason.map(str::to_string),
            stop_sequence: None,
            stop_details: None,
            usage: usage(7),
            request_id: None,
            thought_signature: thought_signature.map(str::to_string),
            reasoning_replay: None,
            context_management: None,
        }
    }

    /// Event order is fixed: the provider's model id leads (cost attribution
    /// for the whole turn), then each block in order (a `tool_use` flushed
    /// right after its block), then `Usage`, then `StopReason` (non-empty),
    /// then `MessageStop`. This is the parity contract the sub-agent provider
    /// client and the TUI fallback both rely on.
    #[test]
    fn response_to_events_preserves_block_then_trailer_order() {
        let events = response_to_events(response(
            vec![
                ::api::OutputContentBlock::Text {
                    text: "hello".to_string(),
                },
                ::api::OutputContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "a.rs"}),
                },
            ],
            Some("tool_use"),
            None,
        ));

        assert_eq!(events.len(), 6, "{events:?}");
        assert!(matches!(&events[0], AssistantEvent::Model(m) if m == "test-model"));
        assert!(matches!(&events[1], AssistantEvent::TextDelta(t) if t == "hello"));
        assert!(matches!(
            &events[2],
            AssistantEvent::ToolUse { id, name, input }
                if id == "t1" && name == "read_file" && input == "{\"path\":\"a.rs\"}"
        ));
        assert!(matches!(&events[3], AssistantEvent::Usage(_)));
        assert!(matches!(&events[4], AssistantEvent::StopReason(r) if r == "tool_use"));
        assert!(matches!(&events[5], AssistantEvent::MessageStop));
    }

    /// An empty or absent `stop_reason` emits no `StopReason` event — only
    /// `Usage` then `MessageStop` trail a plain text turn.
    #[test]
    fn response_to_events_omits_empty_or_absent_stop_reason() {
        for stop in [None, Some("")] {
            let events = response_to_events(response(
                vec![::api::OutputContentBlock::Text {
                    text: "x".to_string(),
                }],
                stop,
                None,
            ));
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, AssistantEvent::StopReason(_))),
                "stop={stop:?} should emit no StopReason: {events:?}"
            );
            assert!(matches!(events.last(), Some(AssistantEvent::MessageStop)));
        }
    }

    /// A present `thought_signature` is surfaced as a Gemini provider-state
    /// blob between `Usage` and `MessageStop`.
    #[test]
    fn response_to_events_surfaces_thought_signature_provider_state() {
        let events = response_to_events(response(
            vec![::api::OutputContentBlock::Text {
                text: "y".to_string(),
            }],
            Some("end_turn"),
            Some("SIG_PARITY"),
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            AssistantEvent::ProviderState(state)
                if state.as_gemini_thought_signature() == Some("SIG_PARITY")
        )));
    }

    #[test]
    fn flush_pending_tool_events_drains_streamed_tool_input() {
        let mut events = Vec::new();
        let mut pending_tools = BTreeMap::new();
        pending_tools.insert(
            0,
            (
                "toolu_1".to_string(),
                "grep_search".to_string(),
                r#"{"pattern":"json.Marshal","head_limit":60}"#.to_string(),
            ),
        );

        flush_pending_tool_events(&mut events, &mut pending_tools);

        assert!(pending_tools.is_empty());
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantEvent::ToolUse { id, name, input }
                if id == "toolu_1"
                    && name == "grep_search"
                    && input.contains("json.Marshal")
                    && input.contains("head_limit")
        ));
    }

    /// `push_output_block` captures a COMPLETE thinking block (non-streaming
    /// path) but NOT the empty streaming placeholder (its text/signature arrive
    /// as deltas the streaming callers accumulate). Redacted thinking arrives
    /// complete on every path, so it is captured either way.
    #[test]
    fn push_output_block_captures_thinking_only_on_the_non_streaming_path() {
        // Non-streaming: complete block captured with its signature.
        let mut events = Vec::new();
        let mut pending: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
        push_output_block(
            ::api::OutputContentBlock::Thinking {
                thinking: "reason".to_string(),
                signature: Some("SIG".to_string()),
            },
            0,
            &mut events,
            &mut pending,
            false,
        );
        assert!(matches!(
            events.as_slice(),
            [AssistantEvent::Thinking { thinking, signature }]
                if thinking == "reason" && signature.as_deref() == Some("SIG")
        ));

        // Streaming: the empty placeholder is not captured here.
        let mut events = Vec::new();
        push_output_block(
            ::api::OutputContentBlock::Thinking {
                thinking: String::new(),
                signature: None,
            },
            0,
            &mut events,
            &mut pending,
            true,
        );
        assert!(
            events.is_empty(),
            "streaming thinking start must not be captured by push_output_block: {events:?}"
        );

        // Redacted thinking is captured on the streaming path too (no deltas).
        let mut events = Vec::new();
        push_output_block(
            ::api::OutputContentBlock::RedactedThinking { data: json!("BLOB") },
            0,
            &mut events,
            &mut pending,
            true,
        );
        assert!(matches!(
            events.as_slice(),
            [AssistantEvent::RedactedThinking { data }] if data == "BLOB"
        ));
    }

    /// The streaming placeholder rule: a `tool_use` whose `input` is an empty
    /// object buffers as `""` when `streaming_tool_input` is set (its real
    /// arguments arrive via `input_json_delta`), but a non-empty object is
    /// stringified immediately. The non-streaming path (`false`) always
    /// stringifies, even for `{}`.
    #[test]
    fn push_output_block_applies_streaming_placeholder_rule() {
        let mut events = Vec::new();
        let mut pending: BTreeMap<u32, (String, String, String)> = BTreeMap::new();

        push_output_block(
            ::api::OutputContentBlock::ToolUse {
                id: "a".to_string(),
                name: "n".to_string(),
                input: json!({}),
            },
            0,
            &mut events,
            &mut pending,
            true,
        );
        assert_eq!(pending.get(&0).map(|t| t.2.as_str()), Some(""));

        push_output_block(
            ::api::OutputContentBlock::ToolUse {
                id: "b".to_string(),
                name: "n".to_string(),
                input: json!({}),
            },
            1,
            &mut events,
            &mut pending,
            false,
        );
        assert_eq!(pending.get(&1).map(|t| t.2.as_str()), Some("{}"));
    }

    // --- prompt_cache_record_to_event: the seam shared by both the
    // Anthropic client path (response_events.rs's `push_prompt_cache_record`)
    // and the non-Anthropic path (`record_non_anthropic_prompt_cache_usage`
    // below) — both hand a `PromptCacheRecord` here and get an
    // `Option<PromptCacheEvent>` back. ---

    #[test]
    fn prompt_cache_record_to_event_is_none_without_break_or_warning() {
        let record = ::api::PromptCacheRecord {
            cache_break: None,
            stats: ::api::PromptCacheStats::default(),
            low_cache_hit_warning: None,
        };
        assert!(prompt_cache_record_to_event(record).is_none());
    }

    /// A session whose cache ratio *stays* low without ever freshly dropping
    /// never trips `detect_cache_break` (no minimum-token drop occurs) — the
    /// "quiet leak" failure mode the low-cache-hit-ratio streak exists to
    /// catch. The converter must still emit an event carrying the warning,
    /// even with `cache_break: None`.
    #[test]
    fn prompt_cache_record_to_event_emits_warning_only_event() {
        let record = ::api::PromptCacheRecord {
            cache_break: None,
            stats: ::api::PromptCacheStats::default(),
            low_cache_hit_warning: Some(
                "prompt cache degraded: 3 consecutive requests re-billed ~180k tokens"
                    .to_string(),
            ),
        };
        let event =
            prompt_cache_record_to_event(record).expect("warning-only record must emit an event");
        assert!(!event.unexpected);
        assert_eq!(event.token_drop, 0);
        assert_eq!(
            event.warning.as_deref(),
            Some("prompt cache degraded: 3 consecutive requests re-billed ~180k tokens")
        );
    }

    #[test]
    fn prompt_cache_record_to_event_carries_warning_alongside_a_break() {
        let record = ::api::PromptCacheRecord {
            cache_break: Some(::api::CacheBreakEvent {
                unexpected: true,
                reason: "cache read tokens dropped while prompt fingerprint remained stable"
                    .to_string(),
                previous_cache_read_input_tokens: 6_000,
                current_cache_read_input_tokens: 1_000,
                token_drop: 5_000,
                model_changed: false,
                system_changed: false,
                tools_changed: false,
                messages_changed: false,
                messages_truncated: false,
                elapsed_secs: 0,
                diverged_message: None,
                tools_added: Vec::new(),
                tools_removed: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                provider: "anthropic".to_string(),
            }),
            stats: ::api::PromptCacheStats::default(),
            low_cache_hit_warning: Some("prompt cache degraded: 3 consecutive requests re-billed ~180k tokens".to_string()),
        };
        let event =
            prompt_cache_record_to_event(record).expect("break record must emit an event");
        assert!(event.unexpected);
        assert_eq!(event.token_drop, 5_000);
        assert!(event.warning.is_some());
    }

    /// End-to-end through the non-Anthropic seam (spec A/B requirement:
    /// instrumentation must work for GPT/OpenAI-compatible providers, not
    /// only the Anthropic client). Three consecutive low-cache-hit-ratio
    /// requests on the same session must surface exactly one warning, on the
    /// third — proving `record_non_anthropic_prompt_cache_usage` carries the
    /// low-cache-hit streak (computed inside `api::PromptCache`) through to
    /// `AssistantEvent::PromptCache(..).warning`.
    #[test]
    fn non_anthropic_low_cache_hit_streak_surfaces_a_warning_event() {
        let _guard = crate::test_env_lock();
        let temp_root = std::env::temp_dir().join(format!(
            "runtime-prompt-cache-warning-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::env::set_var("ZO_CONFIG_HOME", &temp_root);

        let session_id = "runtime-non-anthropic-low-hit";
        let request = ::api::MessageRequest {
            model: "gpt-5.5".to_string(),
            max_tokens: 128,
            messages: vec![::api::InputMessage::user_text("hello")],
            system: None,
            tools: None,
            tool_choice: None,
            stream: true,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        };
        let low_hit_usage = || TokenUsage {
            input_tokens: 60_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 100,
            output_tokens: 5,
            output_tokens_details: None,
        };

        for _ in 0..2 {
            let mut events = vec![AssistantEvent::Usage(low_hit_usage())];
            record_non_anthropic_prompt_cache_usage(
                session_id,
                ::api::ProviderKind::OpenAi,
                &request,
                &mut events,
            );
            assert!(!events.iter().any(|event| matches!(
                event,
                AssistantEvent::PromptCache(cache_event) if cache_event.warning.is_some()
            )));
        }

        let mut third = vec![AssistantEvent::Usage(low_hit_usage())];
        record_non_anthropic_prompt_cache_usage(
            session_id,
            ::api::ProviderKind::OpenAi,
            &request,
            &mut third,
        );
        let warning = third.iter().find_map(|event| match event {
            AssistantEvent::PromptCache(cache_event) => cache_event.warning.clone(),
            _ => None,
        });
        assert!(
            warning.is_some(),
            "3rd consecutive low-cache-hit request should surface a warning"
        );

        std::fs::remove_dir_all(&temp_root).ok();
        std::env::remove_var("ZO_CONFIG_HOME");
    }
}
