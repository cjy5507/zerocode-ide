//! Anthropic-specific SSE → [`RenderBlock`] parser.
//!
//! This is the *only* file in the crate allowed to name Anthropic
//! event types (`StreamEvent`, `ContentBlockDelta`, `OutputContentBlock`).
//! All outputs are the provider-neutral [`RenderBlock`] types from
//! [`crate::message_stream::types`].
//!
//! Implements `code-rules.md` R6: every [`ContentBlockDelta`] variant is
//! named explicitly — no wildcard arm. `ThinkingDelta` and
//! `SignatureDelta` are translated into [`RenderBlock::Reasoning`]
//! rather than being dropped (the bug at `main.rs:~2032`).

use std::collections::BTreeMap;

use api::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    MessageDeltaEvent, MessageStartEvent, OutputContentBlock, StreamEvent,
};
use core_types::retry_signal::is_capacity_text;
use serde_json::Value;
use tokio::sync::mpsc;

use super::source::EventSource;
use super::tools::{format_tool_result, preview_tool_input};
use crate::conversation::{redacted_thinking_data_to_string, AssistantEvent, ProviderStateBlob};
use crate::message_stream::provider::{StreamError, TurnSummary};
use crate::message_stream::types::{
    BlockId, BlockIdGen, RenderBlock, SystemLevel, ToolCallId, ToolCallStatus, ToolPreview,
    ToolResultBody,
};
use crate::usage::TokenUsage;

/// Per-content-block accumulator state.
///
/// Anthropic streams each content block's argument JSON in chunks via
/// `InputJsonDelta` — we aggregate until the block stops, then emit a
/// single running [`RenderBlock::ToolCall`]. Tool blocks that arrive with a
/// complete non-empty input on `content_block_start` can render as `Running`
/// immediately and skip the terminal duplicate update.
#[derive(Debug)]
enum BlockState {
    Text {
        id: BlockId,
    },
    Reasoning {
        id: BlockId,
        /// Buffered signature, set by a `SignatureDelta` arriving
        /// between the text deltas and the block stop.
        signature: Option<String>,
    },
    ToolUse {
        id: BlockId,
        tool_call_id: ToolCallId,
        name: String,
        /// Accumulated `input_json_delta` chunks.
        partial_json: String,
        /// True when the initial `content_block_start` already produced the
        /// final visible Running row. Any later JSON delta clears this so the
        /// stop event refreshes the preview from the completed buffer.
        running_sent_on_start: bool,
        /// `partial_json.len()` at the last liveness progress row emitted while
        /// the tool input was still streaming. Lets `apply_delta` re-send a
        /// throttled Pending update (so a long tool-input stream does not look
        /// frozen) without emitting one render block per tiny delta.
        progress_sent_bytes: usize,
    },
    /// Non-streaming blocks seen on `content_block_start` — nothing to
    /// accumulate. We still need the variant so `content_block_stop`
    /// can find it.
    Inert,
}

/// Parse a fully collected stream of Anthropic [`StreamEvent`]s into
/// [`RenderBlock`]s pushed through `out`.
///
/// Returns a [`TurnSummary`] when `MessageStop` is observed (or the
/// stream ends cleanly). Errors propagate via [`StreamError`].
///
/// This is the legacy synchronous-iterator entry point retained for
/// L1's snapshot tests; production code (L7b+) drives the parser
/// through [`parse_stream_async`] over a real
/// [`crate::message_stream::anthropic::source::EventSource`].
pub async fn parse_stream<I>(
    events: I,
    out: mpsc::Sender<RenderBlock>,
    ids: BlockIdGen,
) -> Result<TurnSummary, StreamError>
where
    I: IntoIterator<Item = StreamEvent>,
{
    let mut parser = StreamParser::new();
    for event in events {
        parser.handle_event(event, &out, &ids).await?;
    }
    parser.finish(&out).await
}

/// Parse an async [`EventSource`] into [`RenderBlock`]s pushed through
/// `out`.
///
/// This is the production code path for L7b: the source is an
/// [`super::source::HttpSource`] wrapping a live `api::MessageStream`,
/// and each `next_event().await` resolves one decoded SSE frame.
///
/// **Backpressure** — the parser awaits `out.send()` whenever the
/// downstream channel is full, which in turn pauses calls into
/// `source.next_event()`, which in turn pauses reqwest's body read.
/// No event is ever buffered beyond the per-block accumulator state
/// inside [`StreamParser`].
///
/// **Cancellation** — when the receiver of `out` drops, the next
/// `out.send().await` resolves with `Err(SendError(_))`, which is
/// converted to [`StreamError::ChannelClosed`]. The parser bubbles
/// this up immediately, the caller drops the [`EventSource`], and
/// reqwest aborts the in-flight body read on `Drop`.
/// Maximum number of stream-level reconnection attempts on transport
/// errors before propagating.
const STREAM_RETRY_MAX: u32 = 2;

/// Backoff delays for stream reconnection attempts (1s, 3s).
const STREAM_RETRY_DELAYS: [std::time::Duration; 2] = [
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(3),
];

/// Emit a throttled liveness `ToolCall` row every time a streaming tool input
/// grows by at least this many bytes. Without it a tool whose arguments arrive
/// as a long `input_json_delta` sequence (a multi-KB Write body, a long bash
/// script) shows the initial `Pending` row and then nothing until
/// `content_block_stop` parses the full JSON — the UI looks frozen mid-tool.
/// Sized so a normal small tool input emits no extra rows (it lands whole on
/// stop) while a large one ticks a handful of "receiving input" updates.
const TOOL_INPUT_PROGRESS_STEP_BYTES: usize = 256;

/// Whether a [`StreamError::Transport`] message is a provider-*emitted* error
/// frame (e.g. an SSE `overloaded_error` event surfaced through
/// `ApiError::StreamApi`'s display) rather than a dropped connection.
///
/// The former cannot be recovered by re-polling the same finished stream — the
/// server already closed the turn — so the in-place reconnect loop skips it and
/// lets the outer turn-level retry re-establish a brand-new request. Genuine
/// connection drops carry no provider error text and still get reconnected.
fn is_provider_emitted_error(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    // The `api stream error` prefix is this site's own marker for an
    // `ApiError::StreamApi` surfaced through a `Transport` error; the capacity
    // vocabulary (`overloaded` / `rate limit` / 429 / 529) is shared with the
    // retry layer via `core_types::retry_signal` so a new overload wording is
    // recognised here and in the backoff classifier at the same time.
    lower.contains("api stream error") || is_capacity_text(&lower)
}

pub async fn parse_stream_async<S: EventSource>(
    mut source: S,
    out: mpsc::Sender<RenderBlock>,
    ids: BlockIdGen,
) -> Result<TurnSummary, StreamError> {
    let mut parser = StreamParser::new();
    let mut transport_retries = 0u32;
    let result: Result<(), StreamError> = async {
        loop {
            if out.is_closed() {
                return Err(StreamError::ChannelClosed);
            }
            match source.next_event().await {
                Ok(Some(event)) => {
                    transport_retries = 0; // reset on successful event
                    parser.handle_event(event, &out, &ids).await?;
                }
                Ok(None) => break,
                // A `Transport` error whose payload is a provider-*emitted* error
                // frame (an SSE `overloaded_error`/`rate_limit` event surfaced via
                // `ApiError::StreamApi`'s display) means the server already closed
                // this turn — re-polling the same finished stream can never recover
                // it. Surface it at once so the turn-level retry (`retry_async`)
                // re-establishes a *fresh* request instead of burning the 1s+3s
                // reconnect budget on a dead pipe. Genuine connection drops (no
                // provider error text) still get the in-place reconnect retries.
                Err(err)
                    if err.transport_message().is_some_and(|msg| {
                        transport_retries < STREAM_RETRY_MAX && !is_provider_emitted_error(msg)
                    }) =>
                {
                    let msg = err.transport_message().unwrap_or("transport error");
                    let delay = STREAM_RETRY_DELAYS
                        .get(transport_retries as usize)
                        .copied()
                        .unwrap_or(STREAM_RETRY_DELAYS[STREAM_RETRY_DELAYS.len() - 1]);
                    eprintln!(
                        "[stream] transport error (attempt {}), retrying in {}s: {msg}",
                        transport_retries + 1,
                        delay.as_secs()
                    );
                    tokio::time::sleep(delay).await;
                    transport_retries += 1;
                    // The source may recover on the next poll, so keep looping.
                }
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }
    .await;

    if let Err(err) = result {
        // Flush open blocks so the TUI gets done:true sentinels
        // even when the stream fails mid-way.
        parser.flush_open_blocks_on_error(&out).await;
        return Err(err);
    }
    parser.finish(&out).await
}

/// Output of the L7c-1b parser variant: both the provider-neutral
/// [`TurnSummary`] **and** the full `AssistantEvent` sequence that the
/// runtime's `build_assistant_message` / session bookkeeping relies on.
///
/// The parser already consumes every [`StreamEvent`] from the source;
/// this type simply stops discarding the information needed to rebuild
/// the [`AssistantEvent`] view, so the L7c `AsyncApiClient`
/// implementation can satisfy both the TUI channel (via `out`) and the
/// runtime bookkeeping (via the returned events) from one SSE stream.
#[derive(Debug, Default)]
pub struct StreamOutputs {
    /// Neutral turn summary identical to [`parse_stream_async`].
    pub summary: TurnSummary,
    /// Accumulated [`AssistantEvent`] sequence mirroring the legacy
    /// CLI collector at `zo_cli::main::AnthropicRuntimeClient::stream`.
    pub events: Vec<AssistantEvent>,
}

/// Async parser variant that co-emits [`RenderBlock`]s (through `out`)
/// and collects the parallel [`AssistantEvent`] sequence required by
/// [`crate::conversation::ConversationRuntime::run_turn_streaming`]'s
/// bookkeeping path.
///
/// This is the L7c-1b runtime helper: the L7c `AsyncApiClient`
/// implementation wraps an [`EventSource`] (normally an
/// [`super::source::HttpSource`] over `api::MessageStream`) and calls
/// this function once per turn. The parser is driven exactly like
/// [`parse_stream_async`] — every byte of `RenderBlock` output is
/// byte-identical — while a second pass on the same `StreamEvent`
/// translates Anthropic wire events into the provider-neutral
/// [`AssistantEvent`] shape so no separate SSE consumption is required.
///
/// # Backpressure & cancellation
///
/// Identical to [`parse_stream_async`]: awaits on `out.send()` honour
/// the bounded-channel contract (R8); a dropped `RenderBlock` receiver
/// short-circuits the loop on the next send with
/// [`StreamError::ChannelClosed`] and `source` is dropped, which in
/// turn aborts the in-flight HTTP body read inside the `api` crate.
///
/// # Ordering guarantees
///
/// * Text deltas land on `out` **before** they are pushed into
///   `StreamOutputs::events` — that matches the live-streaming feel the
///   TUI needs.
/// * [`AssistantEvent::MessageStop`] is appended iff the upstream sent
///   a `MessageStop` frame. The caller is responsible for any
///   tail-patching (e.g. the legacy fallback-to-non-streaming path
///   retained in `main.rs` for defensive compatibility).
pub async fn parse_stream_async_with_events<S: EventSource>(
    mut source: S,
    out: mpsc::Sender<RenderBlock>,
    ids: BlockIdGen,
) -> Result<StreamOutputs, StreamError> {
    let mut parser = StreamParser::new();
    let mut events: Vec<AssistantEvent> = Vec::new();
    // Tool-use blocks keyed by their stream content-block index. The OpenAI
    // Responses backend (gpt-5.5) interleaves parallel function calls across
    // indices, so a single in-flight slot would splice their argument JSON
    // into one malformed call; track each index independently, mirroring the
    // render-side `StreamParser::blocks`.
    let mut pending_tools: BTreeMap<u32, PendingToolUse> = BTreeMap::new();
    // Thinking blocks keyed by content-block index, accumulated across
    // `ThinkingDelta`/`SignatureDelta` and flushed as an `AssistantEvent` on the
    // block's stop so the reasoning is stored (and later replayed) in order.
    let mut pending_thinking: BTreeMap<u32, PendingThinking> = BTreeMap::new();
    // Accumulates the input side (from `message_start`) and output side
    // (from `message_delta`) of token usage so the emitted `Usage` event
    // carries a complete, provider-accurate count rather than output-only.
    let mut usage_acc = TokenUsage::default();
    let mut events_since_yield = 0usize;

    // Unified rate-limit headers are known the instant the stream opens, so
    // surface the 5h/7d snapshot once up-front — the HUD gauges populate
    // before the first token. A closed channel just means the turn is already
    // unwinding; ignore it.
    if let Some(rate_limit) = source.rate_limit() {
        let _ = out.send(RenderBlock::RateLimit(rate_limit)).await;
    }

    let result: Result<(), StreamError> = async {
        loop {
            if out.is_closed() {
                return Err(StreamError::ChannelClosed);
            }
            match source.next_event().await? {
                Some(event) => {
                    // Tee into the AssistantEvent collector BEFORE the
                    // render-side parser consumes the event by value.
                    collect_assistant_event(
                        &event,
                        &mut events,
                        &mut pending_tools,
                        &mut pending_thinking,
                        &mut usage_acc,
                    );
                    // The instant `message_start` lands we already know the
                    // request's input side (prompt + cache read/creation) —
                    // the bulk of context-window occupancy. Forward it as a
                    // live ctx snapshot so the HUD ledger moves *with* the
                    // response instead of snapping only when the turn closes.
                    // Output tokens aren't context, so ctx is final here; the
                    // empty `cumulative` marks this as "ctx-only, cost not yet
                    // known" so the sink updates ctx without zeroing cost.
                    if matches!(event, StreamEvent::MessageStart(_)) {
                        let ctx = usage_acc.context_tokens();
                        if ctx > 0
                            && out
                                .send(RenderBlock::Usage {
                                    ctx_tokens: u64::from(ctx),
                                    cumulative: TokenUsage::default(),
                                    // ctx-only snapshot: cost & breakdown not yet
                                    // known, so the TUI early-returns and keeps the
                                    // prior split until the response completes.
                                    current: TokenUsage::default(),
                                })
                                .await
                                .is_err()
                        {
                            return Err(StreamError::ChannelClosed);
                        }
                    }
                    parser.handle_event(event, &out, &ids).await?;
                    events_since_yield = events_since_yield.saturating_add(1);
                    if events_since_yield >= STREAM_COOPERATIVE_YIELD_EVERY {
                        events_since_yield = 0;
                        tokio::task::yield_now().await;
                    }
                }
                None => break,
            }
        }
        Ok(())
    }
    .await;

    if let Err(err) = result {
        parser.flush_open_blocks_on_error(&out).await;
        if terminal_stream_failure_can_preserve_partial_text(&err, &events) {
            events.push(AssistantEvent::MessageStop);
            let _ = out
                .send(RenderBlock::System {
                    id: ids.next(),
                    level: SystemLevel::Warn,
                    text: "Provider stream ended early; saved the partial assistant response in context.".to_string()
                })
                .await;
            return Ok(StreamOutputs {
                summary: TurnSummary::default(),
                events,
            });
        }
        return Err(err);
    }

    flush_pending_tools(&mut events, &mut pending_tools);
    let summary = parser.finish(&out).await?;
    // Surface the provider stop reason as an `AssistantEvent` so the
    // conversation loop (which only sees `events`, not `summary`) can tell a
    // natural turn end from a turn cut off at the output-token limit and
    // continue the latter instead of mistaking truncation for completion.
    if let Some(reason) = summary
        .stop_reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
    {
        events.push(AssistantEvent::StopReason(reason.to_string()));
    }
    Ok(StreamOutputs { summary, events })
}

const STREAM_COOPERATIVE_YIELD_EVERY: usize = 32;

/// One `tool_use` block whose argument JSON is still streaming in.
#[derive(Debug)]
struct PendingToolUse {
    id: String,
    name: String,
    /// Accumulated `input_json_delta` fragments (raw JSON; the caller parses it).
    input: String,
}

/// One `thinking` block whose text/signature are still streaming in. The
/// `content_block_start` ships an empty placeholder; `ThinkingDelta` fragments
/// build the text and a `SignatureDelta` sets the signature, flushed as an
/// [`AssistantEvent::Thinking`] on `content_block_stop`.
#[derive(Debug, Default)]
struct PendingThinking {
    thinking: String,
    signature: Option<String>,
}

fn flush_pending_tools(
    events: &mut Vec<AssistantEvent>,
    pending_tools: &mut BTreeMap<u32, PendingToolUse>,
) {
    for (_, tool) in std::mem::take(pending_tools) {
        events.push(AssistantEvent::ToolUse {
            id: tool.id,
            name: tool.name,
            input: tool.input,
        });
    }
}

/// Translate one Anthropic [`StreamEvent`] into the equivalent
/// [`AssistantEvent`] push(es). Tool-use blocks are keyed by content-block
/// `index` so parallel/interleaved calls (OpenAI Responses backend) keep their
/// arguments separate instead of splicing into one malformed call.
fn terminal_stream_failure_can_preserve_partial_text(
    err: &StreamError,
    events: &[AssistantEvent],
) -> bool {
    let Some(message) = err.transport_message() else {
        return false;
    };
    if !message.to_ascii_lowercase().contains("terminal stream failure") {
        return false;
    }
    let has_text = events.iter().any(|event| match event {
        AssistantEvent::TextDelta(text) => !text.is_empty(),
        AssistantEvent::ToolUse { .. }
        | AssistantEvent::Thinking { .. }
        | AssistantEvent::RedactedThinking { .. }
        | AssistantEvent::Usage(_)
        | AssistantEvent::PromptCache(_)
        | AssistantEvent::StopReason(_)
        | AssistantEvent::ThoughtSignature(_)
        | AssistantEvent::ProviderState(_)
        | AssistantEvent::ReasoningReplay(_)
        | AssistantEvent::Model(_)
        | AssistantEvent::MessageStop => false,
    });
    let has_tool_use = events
        .iter()
        .any(|event| matches!(event, AssistantEvent::ToolUse { .. }));
    has_text && !has_tool_use
}

fn collect_assistant_event(
    event: &StreamEvent,
    events: &mut Vec<AssistantEvent>,
    pending_tools: &mut BTreeMap<u32, PendingToolUse>,
    pending_thinking: &mut BTreeMap<u32, PendingThinking>,
    usage_acc: &mut TokenUsage,
) {
    match event {
        StreamEvent::MessageStart(MessageStartEvent { message }) => {
            // Anthropic reports the *input* side of the turn (prompt +
            // cache read/creation) on `message_start`, but `output_tokens`
            // only on the closing `message_delta`. Capture the input side
            // here so the merged `Usage` event the runtime records carries
            // a complete, provider-accurate count — without this the
            // emitted usage had input=0/cache=0 and the live ledger's ctx
            // and cost were both undercounted (often reading as 0).
            usage_acc.input_tokens = message.usage.input_tokens;
            usage_acc.cache_creation_input_tokens = message.usage.cache_creation_input_tokens;
            usage_acc.cache_read_input_tokens = message.usage.cache_read_input_tokens;
            // The provider names the model that actually serves the response
            // here (every backend that folds a non-streaming payload into a
            // synthetic `message_start` included), so this one seam covers
            // per-model cost attribution for the whole streaming path.
            if !message.model.is_empty() {
                events.push(AssistantEvent::Model(message.model.clone()));
            }
            // Gemini surfaces the turn's `thoughtSignature` at the message level
            // (its backend folds the non-streaming response into a synthetic
            // `message_start`). Carry it through so the assistant turn can echo
            // it back on the next same-provider request; other providers leave
            // this `None` and emit nothing.
            if let Some(signature) = &message.thought_signature {
                events.push(AssistantEvent::ProviderState(
                    ProviderStateBlob::gemini_thought_signature(signature.clone()),
                ));
            }
            // A ChatGPT/Codex non-streaming response (or the deescalated
            // pre-commit recovery request, which folds a complete
            // `MessageResponse` into a synthetic `message_start`) carries its
            // assembled reasoning-replay payload here; a genuine live stream
            // carries it on the closing `message_delta` instead (see below),
            // since the full set of `function_call`s isn't known until then.
            if let Some(replay) = &message.reasoning_replay {
                events.push(AssistantEvent::ReasoningReplay(replay.clone()));
            }
            // Streaming `message_start` carries no content blocks; a
            // non-streaming payload routed here would, and those blocks are
            // already complete, so emit them directly rather than waiting for
            // a stop event that never arrives.
            for block in &message.content {
                emit_complete_output_block(block, events);
            }
        }
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index,
            content_block,
        }) => collect_block_start(*index, content_block, events, pending_tools, pending_thinking),
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent { index, delta }) => match delta {
            ContentBlockDelta::TextDelta { text } => {
                if !text.is_empty() {
                    events.push(AssistantEvent::TextDelta(text.clone()));
                }
            }
            ContentBlockDelta::InputJsonDelta { partial_json } => {
                // Accumulate into the tool block for *this* index. Parallel
                // calls interleave across indices, so a single shared buffer
                // would splice their arguments into one malformed call.
                if let Some(tool) = pending_tools.get_mut(index) {
                    tool.input.push_str(partial_json);
                }
            }
            ContentBlockDelta::ThinkingDelta { thinking } => {
                // Accumulate the reasoning text for *this* index so it can be
                // stored and replayed. (RenderBlock::Reasoning still carries it
                // to the TUI via the parallel render-side parser pass.)
                if let Some(pending) = pending_thinking.get_mut(index) {
                    pending.thinking.push_str(thinking);
                }
            }
            ContentBlockDelta::SignatureDelta { signature } => {
                // The signature is what makes a thinking block replayable — an
                // unsigned block is dropped at lowering.
                if let Some(pending) = pending_thinking.get_mut(index) {
                    pending.signature = Some(signature.clone());
                }
            }
        },
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index }) => {
            // A block index is either a tool or a thinking block, never both.
            if let Some(pending) = pending_thinking.remove(index) {
                events.push(AssistantEvent::Thinking {
                    thinking: pending.thinking,
                    signature: pending.signature,
                });
            }
            if let Some(tool) = pending_tools.remove(index) {
                events.push(AssistantEvent::ToolUse {
                    id: tool.id,
                    name: tool.name,
                    input: tool.input,
                });
            }
        }
        StreamEvent::MessageDelta(MessageDeltaEvent { delta, usage, .. }) => {
            // A streaming Gemini turn surfaces its `thoughtSignature` on the
            // closing delta (it rides on the late `functionCall` parts, so it is
            // unknown at `message_start`). Capture it the same way the start path
            // does; other providers leave it `None` and emit nothing.
            if let Some(signature) = &delta.thought_signature {
                events.push(AssistantEvent::ProviderState(
                    ProviderStateBlob::gemini_thought_signature(signature.clone()),
                ));
            }
            // ChatGPT/Codex assembles the turn's reasoning-replay payload
            // (covering every `function_call` this turn made) only once the
            // authoritative output snapshot is available, which for a live
            // stream is exactly this closing delta.
            if let Some(replay) = &delta.reasoning_replay {
                events.push(AssistantEvent::ReasoningReplay(replay.clone()));
            }
            // The delta carries the final `output_tokens`; fold it into the
            // input/cache figures captured at `message_start` and emit one
            // complete usage snapshot. Anthropic also restates the input
            // counts on the delta when non-zero, so prefer those if present
            // (a mid-stream context shift) and otherwise keep the start
            // values.
            usage_acc.output_tokens = usage.output_tokens;
            usage_acc.output_tokens_details = usage.output_tokens_details;
            if usage.input_tokens > 0 {
                usage_acc.input_tokens = usage.input_tokens;
            }
            if usage.cache_creation_input_tokens > 0 {
                usage_acc.cache_creation_input_tokens = usage.cache_creation_input_tokens;
            }
            if usage.cache_read_input_tokens > 0 {
                usage_acc.cache_read_input_tokens = usage.cache_read_input_tokens;
            }
            events.push(AssistantEvent::Usage(*usage_acc));
        }
        StreamEvent::MessageStop(_) => {
            flush_pending_tools(events, pending_tools);
            events.push(AssistantEvent::MessageStop);
        }
    }
}

/// Emit a **complete** output block (from a non-streaming payload folded into
/// `message_start`) directly into the `AssistantEvent` view — no accumulation,
/// because the block already carries its full text / arguments / signature.
fn emit_complete_output_block(block: &OutputContentBlock, events: &mut Vec<AssistantEvent>) {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                events.push(AssistantEvent::TextDelta(text.clone()));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            events.push(AssistantEvent::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.to_string(),
            });
        }
        OutputContentBlock::Thinking { thinking, signature } => {
            events.push(AssistantEvent::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            });
        }
        OutputContentBlock::RedactedThinking { data } => {
            events.push(AssistantEvent::RedactedThinking {
                data: redacted_thinking_data_to_string(data),
            });
        }
    }
}

/// Translate one `content_block_start` into the `AssistantEvent` view.
///
/// Tool and thinking blocks arrive as streaming placeholders here — their real
/// content follows as deltas — so they are seeded into `pending_tools` /
/// `pending_thinking` and flushed on the matching `content_block_stop`. Text is
/// emitted immediately, and `redacted_thinking` (which has no deltas) is emitted
/// complete.
fn collect_block_start(
    index: u32,
    content_block: &OutputContentBlock,
    events: &mut Vec<AssistantEvent>,
    pending_tools: &mut BTreeMap<u32, PendingToolUse>,
    pending_thinking: &mut BTreeMap<u32, PendingThinking>,
) {
    match content_block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                events.push(AssistantEvent::TextDelta(text.clone()));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            // The streaming `content_block_start` ships a null/empty-object
            // placeholder when real arguments arrive as `input_json_delta`. Seed
            // the conversation buffer the same way as the render path so the final
            // tool input never becomes `null{...}`.
            pending_tools.insert(
                index,
                PendingToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: tool_input_buffer(input),
                },
            );
        }
        OutputContentBlock::Thinking { thinking, signature } => {
            // Streaming ships an empty placeholder; seed the accumulator and let
            // `ThinkingDelta`/`SignatureDelta` fill it before the stop.
            pending_thinking.insert(
                index,
                PendingThinking {
                    thinking: thinking.clone(),
                    signature: signature.clone(),
                },
            );
        }
        OutputContentBlock::RedactedThinking { data } => {
            // Redacted blocks arrive complete on start (no deltas).
            events.push(AssistantEvent::RedactedThinking {
                data: redacted_thinking_data_to_string(data),
            });
        }
    }
}

/// Incremental Anthropic event → [`RenderBlock`] translator.
///
/// Holds the per-content-block accumulator state across calls so that
/// async drivers (the HTTP path in [`parse_stream_async`]) and sync
/// drivers (the iterator path in [`parse_stream`]) can share the same
/// translation logic without duplicating the dispatch table.
#[derive(Debug, Default)]
pub struct StreamParser {
    summary: TurnSummary,
    blocks: std::collections::HashMap<u32, BlockState>,
}

impl StreamParser {
    /// Create an empty parser.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Translate one Anthropic [`StreamEvent`] into zero or more
    /// [`RenderBlock`]s pushed through `out`.
    pub async fn handle_event(
        &mut self,
        event: StreamEvent,
        out: &mpsc::Sender<RenderBlock>,
        ids: &BlockIdGen,
    ) -> Result<(), StreamError> {
        match event {
            StreamEvent::MessageStart(MessageStartEvent { message }) => {
                self.summary.input_tokens = message.usage.input_tokens;
                // A streaming `message_start` carries no content blocks, but a
                // non-streaming payload folded into a synthetic `message_start`
                // (the Gemini/ChatGPT backends, and any provider's fallback)
                // does — and those blocks are already complete. The event/sync
                // paths render them directly; the render path used to read only
                // `input_tokens` here and silently drop the whole turn's text
                // and tool calls, so their diff/markdown never reached the TUI.
                // Render each as a complete start→finish pair, mirroring the
                // other two paths so every provider renders identically.
                self.render_complete_blocks(&message.content, out, ids)
                    .await?;
            }
            StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index,
                content_block,
            }) => {
                let state = start_block(&content_block, ids, out).await?;
                self.blocks.insert(index, state);
            }
            StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent { index, delta }) => {
                apply_delta(index, delta, &mut self.blocks, ids, out).await?;
            }
            StreamEvent::ContentBlockStop(stop) => {
                if let Some(state) = self.blocks.remove(&stop.index) {
                    finish_block(state, out).await?;
                }
            }
            StreamEvent::MessageDelta(MessageDeltaEvent { delta, usage, .. }) => {
                self.summary.stop_reason.clone_from(&delta.stop_reason);
                self.summary.output_tokens = usage.output_tokens;
            }
            StreamEvent::MessageStop(_) => {
                // drain any still-open blocks (defensive; server
                // should have closed them already).
                let open: Vec<_> = self.blocks.drain().map(|(_, state)| state).collect();
                for state in open {
                    finish_block(state, out).await?;
                }
            }
        }
        Ok(())
    }

    /// Render a batch of already-complete content blocks as start→finish pairs.
    ///
    /// A non-streaming response (Gemini/ChatGPT backends, or any provider's
    /// non-stream fallback) folds the whole turn into a synthetic `message_start`
    /// whose `content` is final rather than streamed. Each block is emitted via
    /// the same [`start_block`]/[`finish_block`] helpers a streamed block uses,
    /// so the TUI sees identical `RenderBlock`s regardless of provider or
    /// streaming mode. Indices are synthesized per call (these blocks never
    /// interleave with streamed deltas in the same turn).
    async fn render_complete_blocks(
        &mut self,
        content: &[OutputContentBlock],
        out: &mpsc::Sender<RenderBlock>,
        ids: &BlockIdGen,
    ) -> Result<(), StreamError> {
        for block in content {
            let state = start_block(block, ids, out).await?;
            finish_block(state, out).await?;
        }
        Ok(())
    }

    /// Drain any still-open accumulator state and return the
    /// accumulated [`TurnSummary`]. Called once after the upstream
    /// event source has been fully consumed.
    pub async fn finish(
        mut self,
        out: &mpsc::Sender<RenderBlock>,
    ) -> Result<TurnSummary, StreamError> {
        let open: Vec<_> = self.blocks.drain().map(|(_, state)| state).collect();
        for state in open {
            finish_block(state, out).await?;
        }
        Ok(self.summary)
    }

    /// Best-effort flush of open blocks on error.
    ///
    /// When the stream errors mid-way, text/reasoning blocks that
    /// received deltas but never got a `ContentBlockStop` event are
    /// left dangling in the TUI (no `done: true` sentinel). This
    /// method sends the closing sentinel for each open block so the
    /// TUI can finalize rendering. Channel errors are silently
    /// ignored since the primary error is the stream failure itself.
    pub async fn flush_open_blocks_on_error(&mut self, out: &mpsc::Sender<RenderBlock>) {
        let open: Vec<_> = self.blocks.drain().map(|(_, state)| state).collect();
        for state in open {
            let _ = finish_block(state, out).await;
        }
    }
}

fn tool_input_is_complete_on_start(input: &Value) -> bool {
    !(input.is_null() || matches!(input, Value::Object(map) if map.is_empty()))
}

fn tool_input_buffer(input: &Value) -> String {
    if tool_input_is_complete_on_start(input) {
        input.to_string()
    } else {
        String::new()
    }
}

async fn start_block(
    content_block: &OutputContentBlock,
    ids: &BlockIdGen,
    out: &mpsc::Sender<RenderBlock>,
) -> Result<BlockState, StreamError> {
    match content_block {
        OutputContentBlock::Text { text } => {
            let id = ids.next();
            if !text.is_empty() {
                out.send(RenderBlock::TextDelta {
                    id,
                    text: text.clone(),
                    done: false,
                })
                .await?;
            }
            Ok(BlockState::Text { id })
        }
        OutputContentBlock::Thinking {
            thinking,
            signature,
        } => {
            let id = ids.next();
            if !thinking.is_empty() {
                out.send(RenderBlock::Reasoning {
                    id,
                    text: thinking.clone(),
                    signature: None,
                    done: false,
                })
                .await?;
            }
            Ok(BlockState::Reasoning {
                id,
                signature: signature.clone(),
            })
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            let block_id = ids.next();
            let tool_call_id = ToolCallId(id.clone());
            let preview = preview_tool_input(name, input);
            let summary = preview_summary(&preview);
            let running_sent_on_start = tool_input_is_complete_on_start(input);
            out.send(RenderBlock::ToolCall {
                id: block_id,
                tool_call_id: tool_call_id.clone(),
                name: name.clone(),
                summary,
                preview,
                status: if running_sent_on_start {
                    ToolCallStatus::Running
                } else {
                    ToolCallStatus::Pending
                },
            })
            .await?;
            Ok(BlockState::ToolUse {
                id: block_id,
                tool_call_id,
                name: name.clone(),
                partial_json: tool_input_buffer(input),
                running_sent_on_start,
                progress_sent_bytes: 0,
            })
        }
        OutputContentBlock::RedactedThinking { .. } => {
            // Redacted thinking carries no renderable text; emit an
            // Info system row so it's not silently dropped (R6).
            let id = ids.next();
            out.send(RenderBlock::System {
                id,
                level: SystemLevel::Info,
                text: "[redacted reasoning]".to_string(),
            })
            .await?;
            Ok(BlockState::Inert)
        }
    }
}

async fn apply_delta(
    index: u32,
    delta: ContentBlockDelta,
    blocks: &mut std::collections::HashMap<u32, BlockState>,
    ids: &BlockIdGen,
    out: &mpsc::Sender<RenderBlock>,
) -> Result<(), StreamError> {
    // R6: exhaustive match, every variant named.
    match delta {
        ContentBlockDelta::TextDelta { text } => {
            if let Some(BlockState::Text { id }) = blocks.get(&index) {
                out.send(RenderBlock::TextDelta {
                    id: *id,
                    text,
                    done: false,
                })
                .await?;
            } else {
                // Block start missing — create one on the fly rather
                // than silently dropping the delta.
                let id = ids.next();
                out.send(RenderBlock::TextDelta {
                    id,
                    text,
                    done: false,
                })
                .await?;
                blocks.insert(index, BlockState::Text { id });
            }
        }
        ContentBlockDelta::InputJsonDelta { partial_json } => {
            if let Some(BlockState::ToolUse {
                id,
                tool_call_id,
                name,
                partial_json: buf,
                running_sent_on_start,
                progress_sent_bytes,
            }) = blocks.get_mut(&index)
            {
                buf.push_str(&partial_json);
                *running_sent_on_start = false;
                // Liveness: a long tool input arrives as many `input_json_delta`
                // chunks and the authoritative Running row is only emitted on
                // `content_block_stop` (`finish_block`), so without an interim
                // signal the tool row sits on `Pending` and the UI looks frozen
                // mid-tool. Re-send the SAME row (merged in place by
                // `tool_call_id`) as `Pending` with a byte-count summary whenever
                // the buffer crosses another `TOOL_INPUT_PROGRESS_STEP_BYTES`
                // boundary. The unparsed JSON is NEVER rendered as the preview —
                // only the tool name + bytes received — so a half-written buffer
                // can't garble the row, and the final `finish_block` Running
                // emission is unchanged.
                if buf.len() >= progress_sent_bytes.saturating_add(TOOL_INPUT_PROGRESS_STEP_BYTES) {
                    *progress_sent_bytes = buf.len();
                    let received = buf.len();
                    out.send(RenderBlock::ToolCall {
                        id: *id,
                        tool_call_id: tool_call_id.clone(),
                        name: name.clone(),
                        summary: format!("receiving input… {received} bytes"),
                        preview: ToolPreview::Generic {
                            name: name.clone(),
                            input_summary: format!("receiving input… {received} bytes"),
                        },
                        status: ToolCallStatus::Pending,
                    })
                    .await?;
                }
            }
        }
        ContentBlockDelta::ThinkingDelta { thinking } => {
            if let Some(BlockState::Reasoning { id, .. }) = blocks.get(&index) {
                out.send(RenderBlock::Reasoning {
                    id: *id,
                    text: thinking,
                    signature: None,
                    done: false,
                })
                .await?;
            } else {
                let id = ids.next();
                out.send(RenderBlock::Reasoning {
                    id,
                    text: thinking,
                    signature: None,
                    done: false,
                })
                .await?;
                blocks.insert(
                    index,
                    BlockState::Reasoning {
                        id,
                        signature: None,
                    },
                );
            }
        }
        ContentBlockDelta::SignatureDelta { signature } => {
            if let Some(BlockState::Reasoning {
                signature: slot, ..
            }) = blocks.get_mut(&index)
            {
                *slot = Some(signature);
            }
        }
    }
    Ok(())
}

async fn finish_block(
    state: BlockState,
    out: &mpsc::Sender<RenderBlock>,
) -> Result<(), StreamError> {
    match state {
        BlockState::Text { id } => {
            out.send(RenderBlock::TextDelta {
                id,
                text: String::new(),
                done: true,
            })
            .await?;
        }
        BlockState::Reasoning { id, signature } => {
            out.send(RenderBlock::Reasoning {
                id,
                text: String::new(),
                signature,
                done: true,
            })
            .await?;
        }
        BlockState::ToolUse {
            id,
            tool_call_id,
            name,
            partial_json,
            running_sent_on_start,
            progress_sent_bytes: _,
        } => {
            if running_sent_on_start {
                return Ok(());
            }
            // Parsing + previewing a large tool input (e.g. a multi-KB Write
            // body or bash script) is a synchronous CPU spike on the streaming
            // task. Yield first so the TUI's `render_tick` is polled and the
            // spinner/input stay live across the parse. Mirrors the periodic
            // yield in `parse_stream_async_with_events`.
            tokio::task::yield_now().await;
            let parsed: Value =
                serde_json::from_str(&partial_json).unwrap_or(Value::String(partial_json.clone()));
            let preview = preview_tool_input(&name, &parsed);
            let summary = preview_summary(&preview);
            out.send(RenderBlock::ToolCall {
                id,
                tool_call_id,
                name,
                summary,
                preview,
                status: ToolCallStatus::Running,
            })
            .await?;
        }
        BlockState::Inert => {}
    }
    Ok(())
}

/// Build a typed result body from an Anthropic `tool_result` content
/// block. Called by the agent loop once it has matched the result to
/// its originating `tool_use`.
#[must_use]
pub fn render_tool_result(name: &str, output: &Value, is_error: bool) -> ToolResultBody {
    format_tool_result(name, output, is_error)
}

fn preview_summary(preview: &ToolPreview) -> String {
    super::tools::preview_summary(preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;

    /// Mock event source that yields a configurable sequence of results.
    struct MockSource {
        events: Vec<Result<Option<StreamEvent>, StreamError>>,
        index: usize,
    }

    impl MockSource {
        fn new(events: Vec<Result<Option<StreamEvent>, StreamError>>) -> Self {
            Self { events, index: 0 }
        }
    }

    impl EventSource for MockSource {
        fn next_event<'a>(
            &'a mut self,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<Option<StreamEvent>, StreamError>>
                    + Send
                    + 'a,
            >,
        > {
            let result = if self.index < self.events.len() {
                let r = self.events[self.index].clone();
                self.index += 1;
                r
            } else {
                Ok(None)
            };
            Box::pin(async move { result })
        }
    }

    // StreamError does not derive Clone — implement it manually for tests.
    impl Clone for StreamError {
        fn clone(&self) -> Self {
            match self {
                Self::ChannelClosed => Self::ChannelClosed,
                Self::Transport(s) => Self::Transport(s.clone()),
                Self::ClassifiedTransport {
                    message,
                    provider_error_class,
                } => Self::ClassifiedTransport {
                    message: message.clone(),
                    provider_error_class: *provider_error_class,
                },
                Self::Protocol(s) => Self::Protocol(s.clone()),
                Self::Adapter { provider, message } => Self::Adapter {
                    provider,
                    message: message.clone(),
                },
            }
        }
    }

    #[tokio::test]
    async fn transport_error_retries_then_succeeds() {
        let (tx, mut rx) = mpsc::channel(32);
        let ids = BlockIdGen::default();

        // Source: transport error, then clean end-of-stream.
        let source = MockSource::new(vec![
            Err(StreamError::Transport("connection reset".into())),
            Ok(None),
        ]);

        let result = parse_stream_async(source, tx, ids).await;
        assert!(result.is_ok(), "should recover from transport error");
        // Channel should be empty (no events emitted).
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn transport_error_exhausts_retries() {
        let (tx, _rx) = mpsc::channel(32);
        let ids = BlockIdGen::default();

        // Source: 3 transport errors (exceeds STREAM_RETRY_MAX of 2).
        let source = MockSource::new(vec![
            Err(StreamError::Transport("fail 1".into())),
            Err(StreamError::Transport("fail 2".into())),
            Err(StreamError::Transport("fail 3".into())),
        ]);

        let result = parse_stream_async(source, tx, ids).await;
        assert!(
            matches!(result, Err(StreamError::Transport(_))),
            "should propagate after retries exhausted, got {result:?}"
        );
    }

    #[tokio::test]
    async fn non_transport_error_propagates_immediately() {
        let (tx, _rx) = mpsc::channel(32);
        let ids = BlockIdGen::default();

        let source = MockSource::new(vec![Err(StreamError::Protocol("bad frame".into()))]);

        let result = parse_stream_async(source, tx, ids).await;
        assert!(
            matches!(result, Err(StreamError::Protocol(_))),
            "protocol errors should not be retried"
        );
    }

    #[tokio::test]
    async fn provider_emitted_overload_surfaces_without_blind_reconnect() {
        let (tx, _rx) = mpsc::channel(32);
        let ids = BlockIdGen::default();

        // A server-sent `overloaded_error` SSE frame reaches the parser as a
        // Transport error carrying the provider's error display. Re-polling the
        // same finished stream cannot recover it, so it must surface on the
        // *first* poll without burning the reconnect budget — even though a
        // second (would-be "successful") event sits queued behind it. Without
        // the provider-emitted guard the reconnect loop would consume the error,
        // see the `Ok(None)`, and wrongly report success.
        let source = MockSource::new(vec![
            Err(StreamError::Transport(
                "api stream error (overloaded_error): Overloaded".into(),
            )),
            Ok(None),
        ]);

        let result = parse_stream_async(source, tx, ids).await;
        assert!(
            matches!(result, Err(StreamError::Transport(ref m)) if m.contains("overloaded")),
            "provider-emitted overload must surface immediately for the outer \
             turn-level retry to re-establish a fresh request, got {result:?}"
        );
    }

    #[test]
    fn provider_emitted_error_classifier_distinguishes_drops_from_error_frames() {
        // Provider-emitted error frames (cannot be reconnected away).
        assert!(super::is_provider_emitted_error(
            "api stream error (overloaded_error): Overloaded"
        ));
        assert!(super::is_provider_emitted_error("upstream OVERLOADED"));
        assert!(super::is_provider_emitted_error("rate_limit exceeded"));
        assert!(super::is_provider_emitted_error("rate limit hit"));
        // Genuine connection drops are recoverable by an in-place reconnect.
        assert!(!super::is_provider_emitted_error("connection reset"));
        assert!(!super::is_provider_emitted_error("broken pipe"));
    }

    fn tool_start(index: u32, id: &str) -> StreamEvent {
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index,
            content_block: OutputContentBlock::ToolUse {
                id: id.to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            },
        })
    }

    fn tool_start_with_input(index: u32, id: &str, input: serde_json::Value) -> StreamEvent {
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index,
            content_block: OutputContentBlock::ToolUse {
                id: id.to_string(),
                name: "bash".to_string(),
                input,
            },
        })
    }

    fn args_delta(index: u32, partial_json: &str) -> StreamEvent {
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index,
            delta: ContentBlockDelta::InputJsonDelta {
                partial_json: partial_json.to_string(),
            },
        })
    }

    fn block_stop(index: u32) -> StreamEvent {
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index })
    }

    fn tool_uses(outputs: &StreamOutputs) -> Vec<(&str, &str)> {
        outputs
            .events
            .iter()
            .filter_map(|event| match event {
                AssistantEvent::ToolUse { id, input, .. } => Some((id.as_str(), input.as_str())),
                _ => None,
            })
            .collect()
    }

    fn text_start(index: u32) -> StreamEvent {
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index,
            content_block: OutputContentBlock::Text {
                text: String::new(),
            },
        })
    }

    fn text_delta(index: u32, text: &str) -> StreamEvent {
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index,
            delta: ContentBlockDelta::TextDelta {
                text: text.to_string(),
            },
        })
    }

    #[tokio::test]
    async fn terminal_stream_failure_preserves_partial_text_response() {
        let (tx, mut rx) = mpsc::channel(64);
        let ids = BlockIdGen::default();
        let source = MockSource::new(vec![
            Ok(Some(text_start(0))),
            Ok(Some(text_delta(0, "partial answer"))),
            Err(StreamError::Transport(
                "api stream error: backend reported a terminal stream failure".into(),
            )),
        ]);

        let outputs = parse_stream_async_with_events(source, tx, ids)
            .await
            .expect("partial text terminal failure should be salvaged");

        assert!(outputs
            .events
            .iter()
            .any(|event| matches!(event, AssistantEvent::TextDelta(text) if text == "partial answer")));
        assert!(matches!(
            outputs.events.last(),
            Some(AssistantEvent::MessageStop)
        ));
        let mut warning_text = None;
        while let Ok(block) = rx.try_recv() {
            if let RenderBlock::System { level, text, .. } = block {
                if matches!(level, SystemLevel::Warn)
                    && text.contains("saved the partial assistant response")
                {
                    warning_text = Some(text);
                }
            }
        }
        let warning_text =
            warning_text.expect("salvage must tell the user the response was partial");
        let lower = warning_text.to_lowercase();
        for banned in [
            "continue from",
            "continue here",
            "without restarting",
            "do not restart",
        ] {
            assert!(
                !lower.contains(banned),
                "salvage warning must describe preserved state without visible continuation filler phrase {banned:?}: {warning_text}"
            );
        }
    }

    /// Parallel tool calls on the OpenAI Responses backend (gpt-5.5) interleave
    /// their `content_block` events across distinct indices: both `start`s land
    /// before either `stop`, and the argument deltas arrive interleaved. A
    /// single in-flight tool slot spliced their argument JSON into one
    /// malformed call (`{"command":"pwd"}{"command":"ls"}`), which the bash
    /// tool then ran verbatim — the garbage commands and hang seen in the TUI.
    /// Each index must accumulate independently.
    #[tokio::test]
    async fn complete_tool_input_starts_running_without_pending_frame() {
        let (tx, mut rx) = mpsc::channel(64);
        let ids = BlockIdGen::default();
        let source = MockSource::new(vec![
            Ok(Some(tool_start_with_input(
                1,
                "call_complete",
                serde_json::json!({ "command": "pwd" }),
            ))),
            Ok(Some(block_stop(1))),
            Ok(None),
        ]);

        let outputs = parse_stream_async_with_events(source, tx, ids)
            .await
            .expect("stream should complete");

        assert_eq!(
            tool_uses(&outputs),
            vec![("call_complete", "{\"command\":\"pwd\"}")],
            "complete start input should still reach the conversation loop"
        );
        let tool_statuses = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|block| match block {
                RenderBlock::ToolCall { status, .. } => Some(status),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tool_statuses,
            vec![ToolCallStatus::Running],
            "complete tool starts should not emit a transient Pending row or duplicate Running update"
        );
    }

    #[tokio::test]
    async fn null_placeholder_tool_input_does_not_prefix_delta_json() {
        let (tx, _rx) = mpsc::channel(64);
        let ids = BlockIdGen::default();
        let source = MockSource::new(vec![
            Ok(Some(tool_start_with_input(1, "call_null", serde_json::Value::Null))),
            Ok(Some(args_delta(1, "{\"command\":\"pwd\"}"))),
            Ok(Some(block_stop(1))),
            Ok(None),
        ]);

        let outputs = parse_stream_async_with_events(source, tx, ids)
            .await
            .expect("stream should complete");

        assert_eq!(
            tool_uses(&outputs),
            vec![("call_null", "{\"command\":\"pwd\"}")],
            "null placeholder must not be concatenated ahead of input_json_delta"
        );
    }

    #[tokio::test]
    async fn delta_tool_input_keeps_pending_until_stop() {
        let (tx, mut rx) = mpsc::channel(64);
        let ids = BlockIdGen::default();
        let source = MockSource::new(vec![
            Ok(Some(tool_start(1, "call_delta"))),
            Ok(Some(args_delta(1, "{\"command\":\"pwd\"}"))),
            Ok(Some(block_stop(1))),
            Ok(None),
        ]);

        parse_stream_async_with_events(source, tx, ids)
            .await
            .expect("stream should complete");

        let tool_statuses = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|block| match block {
                RenderBlock::ToolCall { status, .. } => Some(status),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tool_statuses,
            vec![ToolCallStatus::Pending, ToolCallStatus::Running],
            "delta-streamed tool args still need Pending until the final JSON is known"
        );
    }

    #[tokio::test]
    async fn large_streamed_tool_input_emits_progress_before_running() {
        // A long tool input (e.g. a multi-KB Write body) arrives as many
        // `input_json_delta` chunks; without an interim liveness row the tool
        // sits on the single opening `Pending` frame and the UI looks frozen
        // until `content_block_stop` parses the whole buffer and emits Running.
        // Assert at least one *additional* Pending progress row is emitted
        // between the opening Pending and the terminal Running.
        let (tx, mut rx) = mpsc::channel(256);
        let ids = BlockIdGen::default();

        // A >256B JSON body split across several deltas so the
        // TOOL_INPUT_PROGRESS_STEP_BYTES threshold is crossed mid-stream.
        let big = "x".repeat(1800);
        let mut events = vec![Ok(Some(tool_start(1, "call_big")))];
        events.push(Ok(Some(args_delta(1, "{\"command\":\"echo "))));
        for window in [&big[..600], &big[600..1200], &big[1200..]] {
            events.push(Ok(Some(args_delta(1, window))));
        }
        events.push(Ok(Some(args_delta(1, "\"}"))));
        events.push(Ok(Some(block_stop(1))));
        events.push(Ok(None));

        parse_stream_async_with_events(MockSource::new(events), tx, ids)
            .await
            .expect("stream should complete");

        let tool_statuses = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|block| match block {
                RenderBlock::ToolCall { status, .. } => Some(status),
                _ => None,
            })
            .collect::<Vec<_>>();

        let pending_count = tool_statuses
            .iter()
            .filter(|s| matches!(s, ToolCallStatus::Pending))
            .count();
        assert!(
            pending_count >= 2,
            "a large streamed tool input must emit interim Pending progress rows so the \
             UI is not frozen mid-tool (got statuses {tool_statuses:?})"
        );
        assert_eq!(
            tool_statuses.last(),
            Some(&ToolCallStatus::Running),
            "the row must still settle to Running once the input completes"
        );
    }

    #[tokio::test]
    async fn interleaved_parallel_tool_calls_keep_separate_arguments() {
        let (tx, _rx) = mpsc::channel(64);
        let ids = BlockIdGen::default();
        let source = MockSource::new(vec![
            Ok(Some(tool_start(1, "call_a"))),
            Ok(Some(tool_start(2, "call_b"))),
            Ok(Some(args_delta(1, "{\"command\":\"pwd\"}"))),
            Ok(Some(args_delta(2, "{\"command\":\"ls\"}"))),
            Ok(Some(block_stop(1))),
            Ok(Some(block_stop(2))),
            Ok(None),
        ]);

        let outputs = parse_stream_async_with_events(source, tx, ids)
            .await
            .expect("stream should complete");

        assert_eq!(
            tool_uses(&outputs),
            vec![
                ("call_a", "{\"command\":\"pwd\"}"),
                ("call_b", "{\"command\":\"ls\"}"),
            ],
            "parallel tool calls must not splice arguments across indices"
        );
    }

    /// Sequentially bracketed tool calls (Anthropic, and the simple Responses
    /// case) must keep working unchanged: start→delta→stop, then the next.
    #[tokio::test]
    async fn sequential_tool_calls_accumulate_per_index() {
        let (tx, _rx) = mpsc::channel(64);
        let ids = BlockIdGen::default();
        let source = MockSource::new(vec![
            Ok(Some(tool_start(0, "call_a"))),
            Ok(Some(args_delta(0, "{\"command\":\"pwd\"}"))),
            Ok(Some(block_stop(0))),
            Ok(Some(tool_start(1, "call_b"))),
            Ok(Some(args_delta(1, "{\"command\":\"ls\"}"))),
            Ok(Some(block_stop(1))),
            Ok(None),
        ]);

        let outputs = parse_stream_async_with_events(source, tx, ids)
            .await
            .expect("stream should complete");

        assert_eq!(
            tool_uses(&outputs),
            vec![
                ("call_a", "{\"command\":\"pwd\"}"),
                ("call_b", "{\"command\":\"ls\"}"),
            ],
        );
    }
}
