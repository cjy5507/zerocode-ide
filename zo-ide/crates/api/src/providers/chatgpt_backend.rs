//! ChatGPT (OpenAI) subscription backend over the Responses API.
//!
//! When a user signs in with `zo login openai`, their ChatGPT
//! `access_token` is sent to `chatgpt.com/backend-api/codex/responses` — the
//! same backend OpenAI's Codex CLI uses — instead of the public
//! `api.openai.com` Chat Completions endpoint, so usage bills against the
//! ChatGPT subscription rather than API credits.
//!
//! The wire format is the OpenAI Responses API: an `input` item list plus a
//! stream of `response.*` SSE events. That differs from Chat Completions, so
//! this module owns its own request builder (here) and SSE translation
//! (`sse` submodule, added alongside the client).

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use super::{PromptCacheStrategy, shared_http_client};
use crate::error::ApiError;
use core_types::StreamRetryNotice;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    EffortLevel, ImageSource, InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent,
    MessageRequest, MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock,
    ReasoningRequest,
    StreamEvent, SystemBlock, ToolChoice, ToolDefinition, ToolLedgerView, Usage,
};

/// ChatGPT backend Responses endpoint (Codex variant).
pub const CHATGPT_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
/// `originator` header value identifying the Codex Rust client.
pub const ORIGINATOR: &str = "codex_cli_rs";
/// `OpenAI-Beta` header value opting into the experimental Responses surface.
pub const OPENAI_BETA_RESPONSES: &str = "responses=experimental";
/// `User-Agent` the Codex backend expects. Some models are GATED on this
/// client fingerprint: `gpt-5.6-luna` answers 404 "Model not found" to any
/// request whose user agent does not carry the `codex_cli_rs` product token —
/// verified live (2026-07-13) with the same token, body, and headers, only
/// the UA differing; the version and trailing segments are not checked
/// (`codex_cli_rs` alone passes, `zo/0.1.0` 404s). Mirrors the Claude Max
/// OAuth identity-block fingerprint on the Anthropic path. The `zo` suffix
/// keeps the client honestly identifiable server-side.
pub const USER_AGENT: &str = "codex_cli_rs/0.144.1 (Mac OS; arm64) zo";

/// Build the Responses API request body from zo's provider-agnostic
/// [`MessageRequest`].
///
/// `instructions` carries zo's system prompt — the Responses API takes the
/// system role as a top-level field, not as an `input` item. The conversation
/// history becomes the `input` item list. `store` is forced to `false` (the
/// ChatGPT backend rejects stateful requests) and reasoning encrypted content
/// is requested so multi-turn reasoning continuity is preserved.
///
/// 3-arg convenience wrapper around [`build_responses_request_for_session`]
/// (empty session id) for tests that don't exercise the reasoning-replay
/// cache fallback; every production call site knows its session and calls
/// [`build_responses_request_for_session`] directly.
#[cfg(test)]
pub(crate) fn build_responses_request(
    request: &MessageRequest,
    instructions: &str,
    stream: bool,
) -> Value {
    build_responses_request_for_session(request, instructions, stream, "")
}

/// Like [`build_responses_request`], scoping the reasoning-replay cache
/// fallback (see [`reasoning_for_call`]) to `session_id` — used by the
/// production call sites, which know the client's session. Callers that
/// don't exercise the cache fallback (most existing tests) use the 3-arg
/// [`build_responses_request`], which passes an empty session id.
pub(crate) fn build_responses_request_for_session(
    request: &MessageRequest,
    instructions: &str,
    stream: bool,
    session_id: &str,
) -> Value {
    let supports_vision = super::model_supports_vision(&request.model);
    let mut input = Vec::new();
    for message in &request.messages {
        append_input_items(&mut input, message, session_id, supports_vision);
    }

    let (model_id, fast) = chatgpt_model_and_speed(&request.model);
    // `[fast]` is a zo-side service-tier marker, not part of the model's
    // identity, so the catalog lookup runs on the family id it resolved to.
    // Normally a no-op — an OpenAI model's selection id is its served id.
    let model_id = super::wire_model_for_effort(
        &model_id,
        super::effort_rung(request.reasoning_request()),
    )
    .unwrap_or(model_id);
    // Current GPT families routed here support adaptive default effort — see the
    // `ReasoningRequest::Auto` arm below — and may use the low-latency fast
    // (priority) service tier when the alias carries it.
    // NOTE: The Codex Responses backend (chatgpt.com/backend-api/codex/responses)
    // rejects `max_output_tokens` with `400 {"detail":"Unsupported parameter:
    // max_output_tokens"}` — unlike the public `/v1/responses` API, it accepts no
    // request-side output-token cap at all (confirmed against Codex/Kilocode/
    // LiteLLM reports). Sending it (even from `request.max_tokens`) hard-fails the
    // turn, so it must be omitted. Output length is governed server-side; the
    // response's own `max_output_tokens` incomplete-reason still maps to
    // `StopReason::MaxTokens` in `parse_responses_response`.
    let mut payload = json!({
        "model": model_id,
        "instructions": instructions,
        "input": input,
        "store": false,
        "stream": stream,
        "include": ["reasoning.encrypted_content"],
        "prompt_cache_key": super::prompt_cache_key(&model_id, request, session_id),
    });
    if let Some(retention) =
        PromptCacheStrategy::OpenAiPromptCacheKey.prompt_cache_retention(&model_id)
    {
        payload["prompt_cache_retention"] = json!(retention);
    }

    if let Some(tools) = request.tools.as_ref().filter(|tools| !tools.is_empty()) {
        payload["tools"] = Value::Array(tools.iter().map(responses_tool).collect());
        // Honor an explicit `tool_choice` (e.g. a workflow sub-agent forcing
        // `StructuredOutput`) instead of always sending "auto", which silently
        // weakened the forced tool call (BUG-R16). Default stays "auto".
        payload["tool_choice"] = request
            .tool_choice
            .as_ref()
            .map_or_else(|| json!("auto"), responses_tool_choice);
    }

    // Reasoning effort and fast mode are independent controls. Priority:
    // (1) an explicit provider-neutral `request.effort` the caller set (mapped
    // to GPT's supported wire scale; internal Max/Ultra become xhigh); else
    // (2) a legacy thinking budget selects the tier; else (3) current GPT
    // families scale effort to the task and other families omit reasoning
    // (server-side default).
    let reasoning_request = request.reasoning_request();
    let explicit_top_effort = matches!(
        reasoning_request,
        ReasoningRequest::Effort(
            crate::types::EffortLevel::Xhigh
                | crate::types::EffortLevel::Max
                | crate::types::EffortLevel::Ultra
        )
    );
    let effort = match reasoning_request {
        // Use the user's requested reasoning tier as-is on GPT's scale. `/fast`
        // is a serving-priority signal (service_tier), not a reasoning-effort
        // ceiling, so explicit top-tier requests must remain top-tier.
        //
        // `effort_band_ceiling` marks a DYNAMIC band rather than a static pin
        // (Smart mode): `level` is the band floor (Xhigh), and the shared
        // resolver picks the concrete per-request rung — from this request's
        // own difficulty signals — BEFORE the per-model `gpt_for_model`
        // projection, so the rest of this function (and `explicit_top_effort`
        // above, keyed off the still-Xhigh `reasoning_request`) treats the
        // result exactly as if it had been the named effort all along.
        ReasoningRequest::Effort(level) => {
            let level = match request.effort_band_ceiling {
                Some(ceiling) => super::resolve_effort_band(
                    level,
                    ceiling,
                    &request.model,
                    super::band_difficulty_for_request(request),
                ),
                None => level,
            };
            Some(level.gpt_for_model(&request.model))
        }
        ReasoningRequest::BudgetTokens(budget) => {
            Some(super::effort_level_for_budget(budget).gpt_for_model(&request.model))
        }
        // No explicit tier: send no effort and let the model scale its own
        // reasoning. These families expose an adaptive default for exactly this
        // case.
        //
        // A host-side ladder used to fill this in, and it could only guess:
        // difficulty came from a keyword table plus message length, so a short
        // request went out at `low` however large the work behind it was
        // ("build a 3D physics sim of an anvil crushing a car"). The same model
        // answered visibly worse here than under a harness that sends nothing —
        // and no keyword table can be right, because whether a sentence implies
        // an afternoon of work is a judgement about meaning. Omitting the field
        // hands that judgement to the only party that has read the request.
        // Explicit tiers stay reachable through `/effort`.
        ReasoningRequest::Auto => None,
    };
    // Empty-response de-escalation: when the runtime is retrying a turn whose
    // previous attempt produced no visible output (its retry/continuation
    // system reminder is present), a maximum-reasoning request would walk the
    // exact same path — the model spends the whole response window reasoning
    // and ends with zero text again, deterministically. Step those requests
    // down so the retry actually changes the outcome. An explicit top-tier
    // `/effort xhigh|max|ultra|smart` selection is a user-selected top-effort
    // contract and must remain top-tier, including on GPT fast; lower explicit
    // efforts keep the existing retry de-escalation behavior.
    //
    // Attested rather than silent: a retry that works leaves no other trace, so
    // without this the only way to learn whether the ladder ever ran was to
    // reconstruct it from months of transcripts.
    let effort = if empty_response_pressure(request) {
        let stepped = empty_retry_effort(effort, explicit_top_effort);
        if stepped == effort {
            telemetry::attest_declined(
                telemetry::HarnessFeature::EmptyRetryDeescalation,
                if explicit_top_effort {
                    "explicit_top_effort"
                } else {
                    "already_lowest"
                },
            );
        } else {
            telemetry::attest_fired(telemetry::HarnessFeature::EmptyRetryDeescalation);
        }
        stepped
    } else {
        effort
    };
    // gpt-5.6-luna effort contract (user-set, 2026-07-13): luna runs at
    // `xhigh` or above. Applied LAST — after band resolution, dynamic effort,
    // and the empty-retry de-escalation — so nothing steps a luna request
    // back below the floor. `ultra` clamps to `max`: luna's endpoint tops out
    // at `max` (its models-cache entry lists no `ultra` level, unlike
    // sol/terra). The floor deliberately overrides the de-escalation above
    // for luna; the fast-tier model is cheap enough that a top-effort retry
    // beats a below-contract one.
    let effort = if crate::types::model_id_matches_family(&model_id, "gpt-5.6-luna") {
        // A floor applies to the absent case too. Now that an unspecified tier
        // sends no effort at all, `map` alone would let luna run at whatever its
        // adaptive default picks — including below the contract.
        Some(effort.map_or("xhigh", luna_effort_floor))
    } else {
        effort
    };
    if let Some(effort) = effort {
        payload["reasoning"] = json!({ "effort": effort, "summary": "auto" });
    }

    // "/fast on" (currently encoded by the explicit gpt-5.5-fast alias) is a
    // serving-priority signal, orthogonal to reasoning effort: it asks OpenAI's
    // infrastructure to prioritise the request (~1.5x faster serving, higher
    // credit rate) without changing how hard the model thinks. Do not infer
    // priority from arbitrary future `-fast` suffixes (for example Codex Spark)
    // until they are first-class aliases.
    if fast {
        payload["service_tier"] = json!("priority");
    }

    payload
}

fn build_image_generation_request(
    model: &str,
    prompt: &str,
    size: Option<&str>,
    quality: Option<&str>,
) -> Value {
    let (model, fast) = chatgpt_model_and_speed(model);
    let mut tool = json!({ "type": "image_generation" });
    if let Some(size) = size {
        tool["size"] = json!(size);
    }
    if let Some(quality) = quality {
        tool["quality"] = json!(quality);
    }
    let mut payload = json!({
        "model": model,
        "instructions": "Use the image generation tool exactly once. Do not emit prose.",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": prompt }],
        }],
        "store": false,
        "stream": true,
        "tools": [tool],
        "tool_choice": { "type": "image_generation" },
    });
    if fast {
        payload["service_tier"] = json!("priority");
    }
    payload
}

/// Stable marker prefixes of the runtime's empty-response retry/continuation
/// system reminders (`crates/runtime/src/conversation/mod.rs`). The api crate
/// cannot depend on the runtime crate, so the literals are duplicated here and
/// pinned by tests on both sides.
const EMPTY_RETRY_REMINDER_MARKER: &str = "[zo:empty-response-retry]";
const EMPTY_CONTINUATION_REMINDER_MARKER: &str = "[zo:empty-response-continuation]";

/// Whether this request carries the runtime's empty-response retry or
/// continuation reminder — i.e. the previous attempt at this same context
/// ended with no visible assistant output.
///
/// Both places a reminder can ride are checked, because the runtime moved.
/// Reminders used to be system-prompt text; they are now persisted as a
/// trailing `System` transcript message, which lowers to a `user`-role wire
/// message — so scanning only `system` found nothing on the live path and the
/// de-escalation never once applied. The retry then went back out at the
/// identical effort, walking the identical path, and ended empty again.
///
/// Only the NEWEST message is scanned, and only for a block that BEGINS with a
/// marker. A reminder is its own content block with the marker at offset zero
/// (`wrap_reminder` leaves the already-tagged text alone), while an ordinary
/// transcript can quote these markers verbatim — this file's own source does.
/// A loose `contains` over the whole history would let a conversation about
/// the retry system trigger the retry system.
fn empty_response_pressure(request: &MessageRequest) -> bool {
    let is_marker = |text: &str| {
        text.starts_with(EMPTY_RETRY_REMINDER_MARKER)
            || text.starts_with(EMPTY_CONTINUATION_REMINDER_MARKER)
    };
    let in_system = request.system.as_ref().is_some_and(|blocks| {
        blocks
            .iter()
            .any(|block| matches!(block, SystemBlock::Text { text, .. } if is_marker(text)))
    });
    in_system
        || request.messages.last().is_some_and(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, InputContentBlock::Text { text, .. } if is_marker(text)))
        })
}

/// The rung an empty-response retry goes out at.
///
/// Three cases, and the third is the one this ladder existed on paper for
/// without ever reaching:
/// - an explicit top-tier selection is a user contract and is returned as-is;
/// - a named rung steps down once ([`de_escalated_effort`]);
/// - [`ReasoningRequest::Auto`] resolves to `None` — no effort field at all —
///   so there is no rung to step, and the retry re-sent a request identical to
///   the one that came back empty. That is precisely the case the retry cannot
///   change on its own: the model picked its own reasoning scale, spent the
///   response window on it, and returned nothing; left alone it picks the same
///   scale again.
///
/// Naming a rung here does not contradict the omission rule above ("no keyword
/// table can be right"). That rule is about a FIRST attempt, where the host
/// would be guessing at what the request implies. Here the previous attempt is
/// observed evidence that the model's own choice did not produce output, so
/// stating a rung is a correction to a measured failure, not a guess about
/// meaning. [`EMPTY_RETRY_AUTO_EFFORT`] is the same middle rung the adaptive
/// default lands most turns on, so the correction costs at most one tier.
fn empty_retry_effort(
    effort: Option<&'static str>,
    explicit_top_effort: bool,
) -> Option<&'static str> {
    match effort {
        Some(_) if explicit_top_effort => effort,
        Some(effort) => Some(de_escalated_effort(effort)),
        None => Some(EMPTY_RETRY_AUTO_EFFORT),
    }
}

/// The rung an `Auto` request retries at after coming back empty. Middle of the
/// ladder: low enough that the response window is not spent entirely on
/// reasoning, high enough that a genuinely hard turn is not answered glibly.
const EMPTY_RETRY_AUTO_EFFORT: &str = "medium";

/// One step down the Responses reasoning ladder for an empty-response retry:
/// `ultra` drops to `max`, `max` drops to `high`, `xhigh`/`high` drop to
/// `medium`, and `medium` drops to `low`. Never escalates.
///
/// The `"ultra"` arm closes a wildcard cliff: without it, `"ultra"` fell
/// through to the `_ => "low"` catch-all — a 5-rung drop. It is reachable
/// through an explicit or budget-derived `ultra` whenever that request is not
/// `explicit_top_effort` (band picks are still protected today, so this arm is
/// defense in depth rather than currently load-bearing).
fn de_escalated_effort(effort: &'static str) -> &'static str {
    match effort {
        "ultra" => "max",
        "max" => "high",
        "xhigh" | "high" => "medium",
        _ => "low",
    }
}

/// The `gpt-5.6-luna` reasoning floor: any resolved effort below `xhigh`
/// rises to `xhigh`; `max` is kept (above the floor); `ultra` clamps to `max`
/// because luna's endpoint exposes no `ultra` level. See the call site in
/// [`build_responses_request_for_session`] for why this runs last.
fn luna_effort_floor(effort: &'static str) -> &'static str {
    match effort {
        "max" | "ultra" => "max",
        _ => "xhigh",
    }
}

/// Appended to the system instructions for every model served through this
/// backend (see [`ChatGptBackendClient::instructions`]).
const TOOL_BATCHING_CONTRACT: &str = "\n\n## Tool-call batching contract\n\
Every tool-using response MUST carry every independent tool call you can \
already justify — batch reads, searches, greps, and independent shell checks \
together in one response; three to eight calls at once is normal and \
preferred. A call is independent unless one of its arguments literally \
requires another call's output. Emitting independent calls one per turn is a \
defect, not caution: every extra turn re-sends the entire accumulated \
transcript as billed input. Before ending any tool-using response, ask which \
other calls you already know you need, and add them now.";

fn usage_cached_tokens(usage: Option<&Value>) -> u32 {
    usage
        .and_then(|value| value.pointer("/input_tokens_details/cached_tokens"))
        .and_then(Value::as_u64)
        .and_then(|count| u32::try_from(count).ok())
        .unwrap_or(0)
}

/// Reasoning items to replay immediately before `call_id`'s `function_call`
/// input item. Priority: (1) the message's own attached `reasoning_replay`
/// field — the root fix, populated on every turn produced after this change
/// and persisted with the session so it survives process restarts and
/// sub-agent fanout; (2) the session-scoped replay cache (`reasoning_for_call`)
/// — a defense-in-depth fallback for history that predates the attached
/// field.
fn reasoning_replay_for_call(message: &InputMessage, call_id: &str, session_id: &str) -> Option<Vec<Value>> {
    reasoning_replay_from_attached(message.reasoning_replay.as_ref(), call_id)
        .or_else(|| reasoning_for_call(session_id, call_id))
}

/// Look up `call_id`'s reasoning items inside a message's attached
/// `reasoning_replay` value, shaped
/// `[{"call_id": ..., "items": [...]}, ..., {"turn_final": true, "items": [...]}]`.
/// A turn-final entry carries no `call_id`, so it can never match here.
fn reasoning_replay_from_attached(attached: Option<&Value>, call_id: &str) -> Option<Vec<Value>> {
    attached?
        .as_array()?
        .iter()
        .find(|entry| entry.get("call_id").and_then(Value::as_str) == Some(call_id))
        .and_then(|entry| entry.get("items"))
        .and_then(Value::as_array)
        .cloned()
}

/// Marks the [`reasoning_replay_from_output`] entry whose items belong to the
/// turn's final assistant `message` rather than to a `function_call`. A key
/// rather than a sentinel `call_id` so that pre-existing builds — which match
/// entries by `call_id` alone — skip it instead of ever matching it.
const TURN_FINAL_REPLAY_KEY: &str = "turn_final";

/// Reasoning items to replay immediately before the turn's final assistant
/// `message` input item — the position they occupied in the original response.
///
/// There is no session-cache fallback leg here (unlike
/// [`reasoning_replay_for_call`]): the cache is keyed by `call_id` and a
/// turn-final group has no call to key on, and no synthetic key exists on the
/// wire (an assistant [`InputMessage`] carries no response/item id). The
/// attached field alone is sufficient — see the module note on
/// [`ReasoningReplayStore`]: the cache only ever holds entries this process
/// recorded, and any turn this process recorded also carries the attached
/// field, since both come from the same [`reasoning_replay_from_output`] call.
fn turn_final_reasoning_replay(message: &InputMessage) -> Option<Vec<Value>> {
    message
        .reasoning_replay
        .as_ref()?
        .as_array()?
        .iter()
        .find(|entry| entry.get(TURN_FINAL_REPLAY_KEY).and_then(Value::as_bool) == Some(true))
        .and_then(|entry| entry.get("items"))
        .and_then(Value::as_array)
        .cloned()
}

/// Translate one zo message into zero or more Responses `input` items.
///
/// Assistant turns split into `message` items (text) and `function_call` items
/// (tool calls); user turns become `message` items (text/images) and
/// `function_call_output` items (tool results).
///
/// Reasoning replay is *append-only*: every tool call whose reasoning items
/// are still available (message-attached field first, session-scoped cache
/// fallback second) replays them, as does the turn-final group before the
/// closing assistant `message`, with no recency window or anchor. Any
/// deterministic bound expressed in this layer must eventually *drop* replay
/// items from an old message as the history grows, and that drop is a
/// mid-history mutation that invalidates the provider's prefix cache for the
/// whole suffix — the previous stride-16 staircase anchor re-billed the last
/// 1–2 strides of transcript on every jump (observed live 07-20: warm sol
/// requests re-sending 60–100k uncached at each anchor advance). Replay
/// volume is bounded upstream instead: the runtime's compaction band rewrites
/// the history — dropping summarized messages and their attached replay with
/// them — at exactly the moments the prefix cache is invalidated anyway.
fn append_assistant_input_items(input: &mut Vec<Value>, message: &InputMessage, session_id: &str) {
    let mut text = String::new();
    for block in &message.content {
        match block {
            InputContentBlock::Text { text: value, .. } => text.push_str(value),
            InputContentBlock::ToolUse { .. } => {
                let Some(ToolLedgerView::ToolUse {
                    id,
                    name,
                    input: args,
                }) = ToolLedgerView::from_input_block(block)
                else {
                    unreachable!("tool use block must project to tool use ledger view");
                };
                if !text.is_empty() {
                    input.push(assistant_message(&text));
                    text.clear();
                }
                // Reasoning replay (Codex CLI parity): the stateless Codex
                // backend keeps multi-turn reasoning continuity only when
                // the client re-sends the reasoning items (encrypted
                // content included) that preceded this tool call. Without
                // them gpt-5.5 re-reasons from scratch on every tool
                // result — slower and weaker than codex desktop.
                // The ablation arm looks the items up and then drops them,
                // so the decline counts replay OPPORTUNITIES suppressed
                // rather than assistant blocks visited — the lookup is a
                // pure read of what the message already carries.
                if let Some(items) = reasoning_replay_for_call(message, id, session_id) {
                    if !telemetry::attest_ablated(telemetry::HarnessFeature::ReasoningReplayCall) {
                        telemetry::attest_fired(telemetry::HarnessFeature::ReasoningReplayCall);
                        input.extend(items);
                    }
                }
                input.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": args.to_string(),
                }));
            }
            // Anthropic reasoning blocks are provider-opaque; the Responses
            // backend has its own encrypted-reasoning replay above and never
            // lowers a stored Anthropic thinking block.
            InputContentBlock::ToolResult { .. }
            | InputContentBlock::Image { .. }
            | InputContentBlock::Document { .. }
            | InputContentBlock::Thinking { .. }
            | InputContentBlock::RedactedThinking { .. } => {}
        }
    }
    if !text.is_empty() {
        // Turn-boundary reasoning replay: reasoning that preceded the
        // turn's final text is replayed here, the position it held in the
        // original response. Without it a turn that ends in text (a plain
        // answer, a plan, an EXEC attempt that stopped to explain) throws
        // its reasoning away and the model re-derives it on every later
        // turn — the same defect the per-call replay above fixes, on the
        // other side of the turn boundary. Gated on non-empty text on
        // purpose: these items are only legal in `input` when the item
        // they preceded follows them, and that item is this message.
        if let Some(items) = turn_final_reasoning_replay(message) {
            if !telemetry::attest_ablated(telemetry::HarnessFeature::ReasoningReplayTurnFinal) {
                telemetry::attest_fired(telemetry::HarnessFeature::ReasoningReplayTurnFinal);
                input.extend(items);
            }
        }
        input.push(assistant_message(&text));
    }
}

/// Translate one zo message into zero or more Responses `input` items.
///
/// Assistant turns split into `message` items (text) and `function_call` items
/// (tool calls); user turns become `message` items (text/images) and
/// `function_call_output` items (tool results).
///
/// Reasoning replay is *append-only*: every tool call whose reasoning items
/// are still available (message-attached field first, session-scoped cache
/// fallback second) replays them, as does the turn-final group before the
/// closing assistant `message`, with no recency window or anchor. Any
/// deterministic bound expressed in this layer must eventually *drop* replay
/// items from an old message as the history grows, and that drop is a
/// mid-history mutation that invalidates the provider's prefix cache for the
/// whole suffix — the previous stride-16 staircase anchor re-billed the last
/// 1–2 strides of transcript on every jump (observed live 07-20: warm sol
/// requests re-sending 60–100k uncached at each anchor advance). Replay
/// volume is bounded upstream instead: the runtime's compaction band rewrites
/// the history — dropping summarized messages and their attached replay with
/// them — at exactly the moments the prefix cache is invalidated anyway.
fn append_input_items(
    input: &mut Vec<Value>,
    message: &InputMessage,
    session_id: &str,
    supports_vision: bool,
) {
    if message.role == "assistant" {
        append_assistant_input_items(input, message, session_id);
        return;
    }

    let mut pending_message_blocks = Vec::new();
    for block in &message.content {
        match block {
            InputContentBlock::Text { .. } | InputContentBlock::Image { .. } => {
                pending_message_blocks.push(block);
            }
            InputContentBlock::ToolResult { .. } => {
                let Some(ToolLedgerView::ToolResult {
                    tool_use_id,
                    content,
                    ..
                }) = ToolLedgerView::from_input_block(block)
                else {
                    unreachable!("tool result block must project to tool result ledger view");
                };
                flush_responses_user_message(input, &mut pending_message_blocks, supports_vision);
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": super::flatten_tool_result_content(content),
                }));
                // `function_call_output.output` is a plain string, so any image
                // the tool returned flattens to the placeholder text
                // `[image image/png]` and the pixels are dropped. Anthropic gets
                // a real image block from the identical tool result, so a
                // screenshot the model asked for reached Claude and reached GPT
                // as seventeen characters — it could not check its own work.
                // Re-attach them as a following user item when the model accepts
                // images, which is the only shape the Responses input accepts images in.
                if supports_vision {
                    let images: Vec<Value> = content
                        .iter()
                        .filter_map(|block| match block {
                            crate::types::ToolResultContentBlock::Image { source } => {
                                Some(responses_image_content(source))
                            }
                            crate::types::ToolResultContentBlock::Text { .. }
                            | crate::types::ToolResultContentBlock::Json { .. } => None,
                        })
                        .collect();
                    if !images.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "user",
                            "content": images,
                        }));
                    }
                }
            }
            InputContentBlock::ToolUse { .. }
            | InputContentBlock::Document { .. }
            | InputContentBlock::Thinking { .. }
            | InputContentBlock::RedactedThinking { .. } => {}
        }
    }
    flush_responses_user_message(input, &mut pending_message_blocks, supports_vision);
}

fn assistant_message(text: &str) -> Value {
    json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": text }],
    })
}

fn flush_responses_user_message(
    input: &mut Vec<Value>,
    blocks: &mut Vec<&InputContentBlock>,
    supports_vision: bool,
) {
    if blocks.is_empty() {
        return;
    }

    let content = blocks
        .iter()
        .filter_map(|block| match block {
            InputContentBlock::Text { text, .. } => Some(json!({
                "type": "input_text",
                "text": text,
            })),
            InputContentBlock::Image { source, .. } => {
                if supports_vision {
                    Some(responses_image_content(source))
                } else {
                    Some(json!({
                        "type": "input_text",
                        "text": super::image_omitted_placeholder(&source.media_type),
                    }))
                }
            }
            InputContentBlock::Document { .. }
            | InputContentBlock::ToolUse { .. }
            | InputContentBlock::ToolResult { .. }
            | InputContentBlock::Thinking { .. }
            | InputContentBlock::RedactedThinking { .. } => None,
        })
        .collect::<Vec<_>>();
    blocks.clear();

    if !content.is_empty() {
        input.push(json!({
            "type": "message",
            "role": "user",
            "content": content,
        }));
    }
}

fn responses_image_content(source: &ImageSource) -> Value {
    json!({
        "type": "input_image",
        "image_url": super::image_data_url(source),
        "detail": "auto",
    })
}

/// Flatten a tool-result content list into a single string, matching the
/// Chat Completions path's behaviour (the `function_call_output.output` field
/// is a plain string).
fn responses_tool(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
        "strict": false,
    })
}

/// Map zo's [`ToolChoice`] onto the **Responses API** shape. Unlike Chat
/// Completions (`{"type":"function","function":{"name":..}}`), the Responses API
/// forces a named function with the flat form `{"type":"function","name":..}` —
/// matching [`responses_tool`] above (verified against the OpenAI function-calling
/// reference). `Any` maps to "required" so a forced `StructuredOutput` is honored.
fn responses_tool_choice(tool_choice: &ToolChoice) -> Value {
    match tool_choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Any => json!("required"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool { name } => json!({ "type": "function", "name": name }),
    }
}

/// Map zo's extended-thinking budget onto a Responses `reasoning.effort`
/// bucket using the same shared fallback thresholds as the production request
/// builder, in the legacy GPT-5.5 projection used by this test helper. This
/// clamps every budget at or above the `Max` threshold (24k) down to `xhigh`
/// on legacy GPT-5.5, since this budget-only fallback has no `Ultra` bucket
/// because the OpenAI wire enum does not accept literal `max`/`ultra` values.
/// GPT-5.6 uses the same final wire ceiling even when Zo selects one of
/// those higher internal tiers.
#[cfg(test)]
fn reasoning_effort(budget_tokens: u32) -> &'static str {
    super::effort_level_for_budget(budget_tokens).gpt_for_model("gpt-5.5")
}

/// Split a zo model id into the id the ChatGPT backend is asked for and
/// whether the low-latency `fast` (priority) service tier goes with it.
///
/// A priority spelling is whatever the model's catalog row declares —
/// `gpt-5.6-terra[fast]` (the `/fast` toggle's bracket convention) or the
/// legacy bare `gpt-5.5-fast` — and it folds back to the row's base id so the
/// tier never reaches the wire as part of the model id. Every other id passes
/// through unchanged: a dated id (`gpt-5.5-2026-04-23`) is mapped to its
/// served id by the catalog's `wire` field downstream, and a custom/manual
/// id fails at the provider boundary instead of being rewritten. An arbitrary
/// (unregistered) `-fast` suffix is deliberately NOT a priority alias.
fn chatgpt_model_and_speed(model: &str) -> (String, bool) {
    let (base, fast) = match super::openai_fast_variant_pair(model) {
        Some((base, fast)) if model.trim().eq_ignore_ascii_case(&fast) => (base, true),
        _ => (model.to_string(), false),
    };
    // A qualified spelling of a catalog id (dated, or an unregistered suffix
    // such as a Codex `-fast` the catalog never declared) is asked for as the
    // family id it belongs to.
    (super::catalog_family_id(&base).unwrap_or(base), fast)
}

/// How many `(call_id, reasoning items)` pairs one session's replay cache
/// queue retains. Widened from the old process-global 64 now that the cache
/// is partitioned per session (see [`ReasoningReplayStore`]) instead of
/// shared FIFO-evicted across every concurrent sub-agent/session in the
/// process — a single session can afford a deeper queue since it no longer
/// competes with unrelated sessions for the same slots.
const REASONING_REPLAY_CAP: usize = 256;

/// How many distinct sessions [`ReasoningReplayStore`] retains before
/// evicting the oldest session's whole queue. Bounds total process memory
/// across a long-running process that opens many sessions (sub-agent
/// fanout, `--resume`, headless batch runs), the exact scenario that used to
/// evict the *main* conversation's cache entries out of the old
/// process-global FIFO.
/// Sized to 8 to respect low-end RAM ceilings and realistic concurrent sessions.
const REASONING_REPLAY_MAX_SESSIONS: usize = 8;

/// FIFO entries of one session's reasoning replay cache: `(call_id, reasoning items)`.
type ReasoningReplayEntries = VecDeque<(String, Vec<Value>)>;

/// Session-scoped replay store for Responses reasoning items, keyed by
/// `session_id` then by the `call_id` of the `function_call` that followed
/// the items in the same response.
///
/// The Codex backend is stateless (`store: false`): reasoning continuity
/// across turns exists only if the client *replays* the reasoning items —
/// `encrypted_content` included — in the next request's input, which is what
/// Codex CLI/desktop does. This store is the defense-in-depth fallback for
/// history that predates [`InputMessage::reasoning_replay`] (the root fix —
/// see `append_input_items`); it used to be a single process-wide FIFO queue,
/// which meant a sub-agent fanout's calls could evict the *main*
/// conversation's still-needed entries. Partitioning by session id isolates
/// each session's queue so unrelated sessions can no longer starve each
/// other; `session_order` tracks session insertion order so once more than
/// [`REASONING_REPLAY_MAX_SESSIONS`] sessions have written to the store, the
/// oldest session's entire queue is evicted (simple whole-session FIFO, not
/// per-entry LRU).
#[derive(Default)]
struct ReasoningReplayStore {
    sessions: std::collections::HashMap<String, ReasoningReplayEntries>,
    session_order: VecDeque<String>,
}

impl ReasoningReplayStore {
    fn record(&mut self, session_id: &str, call_id: &str, items: Vec<Value>) {
        if !self.sessions.contains_key(session_id) {
            self.session_order.push_back(session_id.to_string());
            self.sessions.insert(session_id.to_string(), VecDeque::new());
            while self.session_order.len() > REASONING_REPLAY_MAX_SESSIONS {
                let Some(evicted) = self.session_order.pop_front() else {
                    break;
                };
                self.sessions.remove(&evicted);
            }
        }
        // The session just inserted above is always the newest entry in
        // `session_order`, so it cannot have been the one evicted by the
        // FIFO trim; the lookup is still `Option`-guarded rather than
        // indexed/panicking.
        let Some(queue) = self.sessions.get_mut(session_id) else {
            return;
        };
        queue.retain(|(existing, _)| existing != call_id);
        queue.push_back((call_id.to_string(), items));
        while queue.len() > REASONING_REPLAY_CAP {
            queue.pop_front();
        }
    }

    fn lookup(&self, session_id: &str, call_id: &str) -> Option<Vec<Value>> {
        self.sessions
            .get(session_id)?
            .iter()
            .find(|(existing, _)| existing == call_id)
            .map(|(_, items)| items.clone())
    }
}

fn reasoning_replay_store() -> &'static Mutex<ReasoningReplayStore> {
    static STORE: OnceLock<Mutex<ReasoningReplayStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(ReasoningReplayStore::default()))
}

/// Record the reasoning items that preceded `call_id` in a streamed response,
/// scoped to `session_id`. Re-recording the same `call_id` (stream restart)
/// replaces the old entry.
fn cache_reasoning_for_call(session_id: &str, call_id: &str, items: Vec<Value>) {
    if call_id.is_empty() || items.is_empty() {
        return;
    }
    reasoning_replay_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record(session_id, call_id, items);
}

/// The reasoning items to replay immediately before `call_id`'s
/// `function_call` input item, if this process produced them for `session_id`.
/// Legacy fallback — see [`ReasoningReplayStore`] — consulted only when the
/// message itself carries no attached `reasoning_replay` payload.
fn reasoning_for_call(session_id: &str, call_id: &str) -> Option<Vec<Value>> {
    reasoning_replay_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .lookup(session_id, call_id)
}

#[cfg(test)]
fn remove_reasoning_for_call(session_id: &str, call_id: &str) {
    let mut store = reasoning_replay_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(queue) = store.sessions.get_mut(session_id) {
        queue.retain(|(existing, _)| existing != call_id);
    }
}

/// Incremental parser turning the ChatGPT backend's Responses SSE byte stream
/// into event JSON values. Frames are `data: {json}` blocks separated by blank
/// lines; each payload carries a `"type"` discriminator consumed by
/// [`ResponsesStreamState`].
#[derive(Debug, Default)]
pub(crate) struct ResponsesSseParser {
    buffer: Vec<u8>,
    /// How far into `buffer` the frame-separator search has already looked.
    /// Without this, every `push` rescans the whole accumulated buffer from
    /// byte 0 — O(n²) over a frame that arrives in many chunks. The ChatGPT
    /// backend streams a large `reasoning.encrypted_content` blob as a single
    /// SSE frame split across hundreds of TCP chunks, so the quadratic scan
    /// pegged a CPU core for seconds and froze the TUI event loop (the
    /// "gpt-5.5 stutters/freezes, Claude is fine" report). Resuming the scan
    /// from where the last one stopped makes total work linear in bytes seen.
    scanned: usize,
}

impl ResponsesSseParser {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; return every complete event JSON value it now contains.
    ///
    /// Errors (following the Anthropic `SseParser` contract) when the retained
    /// partial-frame buffer would exceed the crate-wide SSE cap, so a stream
    /// that never emits a separator cannot grow memory without limit. The cap is
    /// high enough for the large `reasoning.encrypted_content` frame the Codex
    /// Responses backend streams as one event.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, ApiError> {
        crate::sse::guard_sse_buffer_push(self.buffer.len(), chunk.len())?;
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(frame) = self.next_frame() {
            if let Some(value) = parse_data_frame(&frame) {
                events.push(value);
            }
        }
        Ok(events)
    }

    /// Pop the next complete `\n\n`- or `\r\n\r\n`-terminated frame, resuming the
    /// separator search from `scanned` rather than the buffer start. A boundary
    /// can straddle the resume point by up to 3 bytes (`\r\n\r\n`), so the scan
    /// starts a few bytes before `scanned`; matched bytes are then drained and
    /// `scanned` rebased to 0.
    fn next_frame(&mut self) -> Option<String> {
        // Back up enough that a separator split across the previous resume
        // point is still found (max separator length is 4 → overlap of 3).
        let start = self.scanned.saturating_sub(3);
        let found = self.buffer[start..]
            .windows(2)
            .position(|window| window == b"\n\n")
            .map(|position| (start + position, 2))
            .or_else(|| {
                self.buffer[start..]
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|position| (start + position, 4))
            });
        let Some((end, sep)) = found else {
            // No frame yet: everything except the trailing 3-byte overlap window
            // has been searched, so the next push resumes from there.
            self.scanned = self.buffer.len().saturating_sub(3);
            return None;
        };
        let frame = String::from_utf8_lossy(&self.buffer[..end]).into_owned();
        self.buffer.drain(..end + sep);
        self.scanned = 0;
        Some(frame)
    }
}

fn parse_data_frame(frame: &str) -> Option<Value> {
    let mut payload = String::new();
    for line in frame.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            if !payload.is_empty() {
                payload.push('\n');
            }
            payload.push_str(data.trim_start());
        }
    }
    if payload.is_empty() || payload == "[DONE]" {
        return None;
    }
    serde_json::from_str(&payload).ok()
}

/// Translates Responses events into zo's Anthropic-shaped [`StreamEvent`]s.
/// The Responses `output_index` maps directly onto zo's content-block index,
/// and the state tracks open blocks so `response.completed` can close any text
/// or reasoning item whose explicit `output_item.done` frame was skipped or
/// reordered.
#[derive(Debug)]
pub(crate) struct ResponsesStreamState {
    model: String,
    /// Session id the reasoning-replay cache fallback is scoped to (see
    /// [`ReasoningReplayStore`]). Empty by default; production callers set it
    /// via [`Self::with_session_id`] when they construct the state.
    session_id: String,
    message_started: bool,
    finished: bool,
    started_blocks: BTreeSet<u32>,
    open_blocks: BTreeSet<u32>,
    /// Completed `reasoning` items (full JSON, `encrypted_content` included)
    /// not yet attributed to a tool call. When a `function_call` item
    /// completes, these are cached under its `call_id` for next-turn replay.
    pending_reasoning: Vec<Value>,
    /// Every completed item this turn streamed, in arrival order — the
    /// reconstruction of `output` for a backend that does not send one.
    ///
    /// Codex runs with `store: false` and its terminal frame carries
    /// `"output": []`, so the "authoritative snapshot" the replay fold reads
    /// is empty on every live turn. This is the same gap
    /// [`completed_response_from_sse`] fills for the answer TEXT, at the layer
    /// where the reasoning items actually arrive: `response.output_item.done`.
    streamed_output: Vec<Value>,
    /// Whether this turn streamed any assistant text. A turn-final reasoning
    /// group is only legal in `input` immediately before the message it
    /// preceded, so the reconstruction may only claim one when a message
    /// really did follow.
    streamed_text: bool,
    /// Assembled `[{"call_id":..,"items":[..]}]` reasoning-replay entries for
    /// this turn, built once at the terminal frame
    /// ([`Self::completed_output_deltas`]) from whichever view of the turn's
    /// output is actually populated, so it reflects every `function_call` in
    /// the turn regardless of which individual `output_item.done` streaming
    /// frames arrived. `None` when the turn produced no replayable reasoning.
    reasoning_replay: Option<Value>,
    text_delta_indices: BTreeSet<u32>,
    input_delta_indices: BTreeSet<u32>,
    /// Terminal failure reported by the backend (`response.failed` / `error`
    /// SSE events). Held here because `ingest` returns display events only;
    /// the stream loop drains it via [`Self::take_failure`] and surfaces it as
    /// an [`ApiError`] instead of silently ending an empty stream.
    failure: Option<ApiError>,
}

impl ResponsesStreamState {
    pub(crate) fn new(model: String) -> Self {
        Self {
            model,
            session_id: String::new(),
            message_started: false,
            finished: false,
            started_blocks: BTreeSet::new(),
            open_blocks: BTreeSet::new(),
            pending_reasoning: Vec::new(),
            streamed_output: Vec::new(),
            streamed_text: false,
            reasoning_replay: None,
            text_delta_indices: BTreeSet::new(),
            input_delta_indices: BTreeSet::new(),
            failure: None,
        }
    }

    /// Scope the reasoning-replay cache fallback to `session_id`. Chainable;
    /// production call sites set it from the client's session immediately
    /// after construction (see [`ChatGptBackendClient::stream_message`]).
    #[must_use]
    pub(crate) fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    /// Drain a terminal backend failure recorded by `ingest`
    /// (`response.failed` / top-level `error` events).
    pub(crate) fn take_failure(&mut self) -> Option<ApiError> {
        self.failure.take()
    }

    pub(crate) fn ingest(&mut self, event: &Value) -> Vec<StreamEvent> {
        if self.finished {
            return Vec::new();
        }
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.created" => self.start_message(event),
            "response.output_item.added" => self.start_item(event),
            "response.output_text.delta" => self.text_delta(event, "delta", false),
            "response.output_text.done" => self.text_delta(event, "text", true),
            "response.content_part.done" => self.content_part_done(event),
            "response.function_call_arguments.delta" => self.function_args_delta(event, false),
            "response.function_call_arguments.done" => self.function_args_delta(event, true),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => self
                .block_delta(
                    event,
                    Some(OutputContentBlock::Thinking {
                        thinking: String::new(),
                        signature: None,
                    }),
                    |delta| ContentBlockDelta::ThinkingDelta { thinking: delta },
                ),
            "response.reasoning_summary_part.added" => self.reasoning_part_boundary(event),
            "response.output_item.done" => self.block_stop(event),
            "response.completed" => self.finish(event, "end_turn"),
            // An incomplete response is still a terminal close: the model ran
            // out of output budget (typically all spent on reasoning) or was
            // filtered. Close the message with an honest stop_reason instead of
            // ignoring the frame — ignoring it ended the stream with zero
            // events, which the runtime mistook for "the model returned no
            // assistant content" and retried the identical request forever.
            "response.incomplete" => {
                let reason = incomplete_stop_reason(event);
                self.finish(event, reason)
            }
            "response.failed" => {
                self.record_failure(
                    event.pointer("/response/error/code"),
                    event.pointer("/response/error/message"),
                    event,
                );
                Vec::new()
            }
            "error" => {
                self.record_failure(event.get("code"), event.get("message"), event);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Record a terminal backend failure (`response.failed` / `error`).
    /// Server-side faults and throttles are retryable (the pre-commit restart
    /// path re-issues the request); invalid-request class codes are not.
    fn record_failure(&mut self, code: Option<&Value>, message: Option<&Value>, event: &Value) {
        if self.finished || self.failure.is_some() {
            return;
        }
        let code = code.and_then(Value::as_str).unwrap_or_default().to_string();
        let message = match message.and_then(Value::as_str) {
            Some(text) => text.to_string(),
            None => match failure_frame_digest(event) {
                Some(digest) => format!("{TERMINAL_STREAM_FAILURE} ({digest})"),
                None => TERMINAL_STREAM_FAILURE.to_string(),
            },
        };
        if code == USAGE_LIMIT_CODE {
            self.failure = Some(usage_limit_error(
                Some(message),
                event.to_string(),
                usage_limit_reset(event, measured_usage_reset),
            ));
            return;
        }
        let retryable = matches!(
            code.as_str(),
            "server_error" | "rate_limit_exceeded" | "overloaded" | "slow_down" | ""
        );
        self.failure = Some(ApiError::StreamApi {
            error_type: (!code.is_empty()).then_some(code),
            message: Some(message.clone()),
            body: message,
            retryable,
        });
    }

    fn start_message(&mut self, event: &Value) -> Vec<StreamEvent> {
        if self.message_started {
            return Vec::new();
        }
        self.message_started = true;
        let id = event
            .pointer("/response/id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        vec![StreamEvent::MessageStart(MessageStartEvent {
            message: MessageResponse {
                id,
                kind: "message".to_string(),
                role: "assistant".to_string(),
                content: Vec::new(),
                model: self.model.clone(),
                stop_reason: None,
                stop_sequence: None,
                usage: zero_usage(),
                request_id: None,
                thought_signature: None,
                reasoning_replay: None,
                context_management: None,
            },
        })]
    }

    fn start_item(&mut self, event: &Value) -> Vec<StreamEvent> {
        let Some(index) = output_index(event) else {
            return Vec::new();
        };
        let item = &event["item"];
        let content_block = match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message" => OutputContentBlock::Text {
                text: String::new(),
            },
            "function_call" => OutputContentBlock::ToolUse {
                id: str_field(item, "call_id"),
                name: str_field(item, "name"),
                input: json!({}),
            },
            "reasoning" => OutputContentBlock::Thinking {
                thinking: String::new(),
                signature: None,
            },
            _ => return Vec::new(),
        };
        if !self.started_blocks.insert(index) {
            return Vec::new();
        }
        self.open_blocks.insert(index);
        vec![StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index,
            content_block,
        })]
    }

    fn text_delta(&mut self, event: &Value, field: &str, final_payload: bool) -> Vec<StreamEvent> {
        let Some(index) = output_index(event) else {
            return Vec::new();
        };
        if final_payload && self.text_delta_indices.contains(&index) {
            return Vec::new();
        }
        let Some(text) = event.get(field).and_then(Value::as_str) else {
            return Vec::new();
        };
        self.emit_text_delta(index, text)
    }

    fn content_part_done(&mut self, event: &Value) -> Vec<StreamEvent> {
        let Some(index) = output_index(event) else {
            return Vec::new();
        };
        if self.text_delta_indices.contains(&index) {
            return Vec::new();
        }
        let part = &event["part"];
        if part.get("type").and_then(Value::as_str) != Some("text") {
            return Vec::new();
        }
        let Some(text) = part.get("text").and_then(Value::as_str) else {
            return Vec::new();
        };
        self.emit_text_delta(index, text)
    }

    fn emit_text_delta(&mut self, index: u32, text: &str) -> Vec<StreamEvent> {
        if text.is_empty() {
            return Vec::new();
        }
        self.text_delta_indices.insert(index);
        // The turn ends in text, so a turn-final reasoning group has a message
        // to sit in front of — see `replay_source`.
        self.streamed_text = true;
        self.emit_delta(
            index,
            Some(OutputContentBlock::Text {
                text: String::new(),
            }),
            ContentBlockDelta::TextDelta {
                text: text.to_string(),
            },
        )
    }

    fn function_args_delta(&mut self, event: &Value, final_payload: bool) -> Vec<StreamEvent> {
        let Some(index) = output_index(event) else {
            return Vec::new();
        };
        if final_payload && self.input_delta_indices.contains(&index) {
            return Vec::new();
        }
        let field = if final_payload { "arguments" } else { "delta" };
        let Some(delta) = event.get(field).and_then(Value::as_str) else {
            return Vec::new();
        };
        let fallback_start = final_payload.then(|| OutputContentBlock::ToolUse {
            id: str_field(event, "call_id"),
            name: str_field(event, "name"),
            input: json!({}),
        });
        self.emit_input_delta(index, delta, fallback_start)
    }

    fn emit_input_delta(
        &mut self,
        index: u32,
        delta: &str,
        fallback_start: Option<OutputContentBlock>,
    ) -> Vec<StreamEvent> {
        if delta.is_empty() {
            return Vec::new();
        }
        self.input_delta_indices.insert(index);
        self.emit_delta(
            index,
            fallback_start,
            ContentBlockDelta::InputJsonDelta {
                partial_json: delta.to_string(),
            },
        )
    }

    /// A new reasoning summary part opened. OpenAI streams summary parts as
    /// separate texts on ONE output item with no separator between them, so
    /// concatenation produced a run-on paragraph — and the TUI's thinking
    /// title (first line of the current paragraph) froze on part 0's topic
    /// for the whole reasoning phase. Emit a paragraph break between parts so
    /// they render, and re-title, as paragraphs. `summary_index == 0` (or a
    /// part on a not-yet-started item) needs no separator.
    fn reasoning_part_boundary(&mut self, event: &Value) -> Vec<StreamEvent> {
        let Some(index) = output_index(event) else {
            return Vec::new();
        };
        let summary_index = event
            .get("summary_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if summary_index == 0 || !self.started_blocks.contains(&index) {
            return Vec::new();
        }
        self.emit_delta(
            index,
            None,
            ContentBlockDelta::ThinkingDelta {
                thinking: "\n\n".to_string(),
            },
        )
    }

    fn block_delta(
        &mut self,
        event: &Value,
        fallback_start: Option<OutputContentBlock>,
        make: impl FnOnce(String) -> ContentBlockDelta,
    ) -> Vec<StreamEvent> {
        let Some(index) = output_index(event) else {
            return Vec::new();
        };
        let Some(delta) = event.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        self.emit_delta(index, fallback_start, make(delta.to_string()))
    }

    fn emit_delta(
        &mut self,
        index: u32,
        fallback_start: Option<OutputContentBlock>,
        delta: ContentBlockDelta,
    ) -> Vec<StreamEvent> {
        self.open_blocks.insert(index);
        let mut events = Vec::new();
        if let Some(content_block) = fallback_start {
            if self.started_blocks.insert(index) {
                events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                    index,
                    content_block,
                }));
            }
        }
        events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index,
            delta,
        }));
        events
    }

    fn block_stop(&mut self, event: &Value) -> Vec<StreamEvent> {
        let Some(index) = output_index(event) else {
            return Vec::new();
        };
        // Reasoning replay capture: `output_item.done` carries the complete
        // item JSON. Hold finished `reasoning` items until the `function_call`
        // they precede completes, then cache them under its `call_id` so the
        // next turn's request can replay them (Codex CLI parity — see
        // `ReasoningReplayStore`). This feeds only the session-scoped cache
        // fallback; the authoritative per-turn `reasoning_replay` payload sent
        // to the runtime is assembled once in `completed_output_deltas`.
        let item = &event["item"];
        // Recorded before the match below consumes anything: these frames are
        // the ONLY place a live Codex turn's reasoning items appear, so the
        // terminal frame's empty `output` can be reconstructed from them.
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "reasoning" | "function_call" | "message" => self.streamed_output.push(item.clone()),
            _ => {}
        }
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "reasoning" => self.pending_reasoning.push(item.clone()),
            "function_call" => {
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                cache_reasoning_for_call(
                    &self.session_id,
                    call_id,
                    std::mem::take(&mut self.pending_reasoning),
                );
            }
            _ => {}
        }
        let mut events = match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message" => self.message_done_delta(index, item),
            "function_call" => self.function_call_done_delta(index, item),
            _ => Vec::new(),
        };
        self.open_blocks.remove(&index);
        self.started_blocks.remove(&index);
        events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
            index,
        }));
        events
    }

    fn message_done_delta(&mut self, index: u32, item: &Value) -> Vec<StreamEvent> {
        if self.text_delta_indices.contains(&index) {
            return Vec::new();
        }
        let Some(parts) = item.get("content").and_then(Value::as_array) else {
            return Vec::new();
        };
        let text = parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<String>();
        self.emit_text_delta(index, &text)
    }

    fn function_call_done_delta(&mut self, index: u32, item: &Value) -> Vec<StreamEvent> {
        if self.input_delta_indices.contains(&index) {
            return Vec::new();
        }
        let Some(arguments) = item.get("arguments").and_then(Value::as_str) else {
            return Vec::new();
        };
        let fallback_start = Some(OutputContentBlock::ToolUse {
            id: str_field(item, "call_id"),
            name: str_field(item, "name"),
            input: json!({}),
        });
        self.emit_input_delta(index, arguments, fallback_start)
    }

    fn finish(&mut self, event: &Value, stop_reason: &str) -> Vec<StreamEvent> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let usage = event.pointer("/response/usage");
        let cache_read_input_tokens = usage_cached_tokens(usage);
        let token_usage = Usage {
            input_tokens: usage_field(usage, "input_tokens")
                .saturating_sub(cache_read_input_tokens),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens,
            output_tokens: usage_field(usage, "output_tokens"),
            output_tokens_details: None,
        };
        let mut events: Vec<StreamEvent> =
            self.completed_output_deltas(event).into_iter().collect();
        events.extend(
            self.open_blocks
                .iter()
                .copied()
                .map(|index| StreamEvent::ContentBlockStop(ContentBlockStopEvent { index })),
        );
        self.open_blocks.clear();
        self.started_blocks.clear();
        self.text_delta_indices.clear();
        self.input_delta_indices.clear();
        events.extend([
            StreamEvent::MessageDelta(MessageDeltaEvent {
                delta: MessageDelta {
                    stop_reason: Some(stop_reason.to_string()),
                    stop_sequence: None,
                    thought_signature: None,
                    reasoning_replay: self.reasoning_replay.take(),
                },
                usage: token_usage,
                context_management: None,
            }),
            StreamEvent::MessageStop(MessageStopEvent {}),
        ]);
        events
    }

    /// The turn's output items to fold a replay payload out of: the terminal
    /// frame's own `output` when that carries reasoning, otherwise what the
    /// stream itself delivered.
    ///
    /// The fallback is not an edge case — it is the LIVE path, and the
    /// condition is REASONING rather than emptiness because the live frame is
    /// not empty. Codex sends `store: false`, so it retains no reasoning to
    /// hand back: its terminal frame carries the turn's `message` and
    /// `function_call` items and no `reasoning` items at all. Folding over
    /// that yields nothing, which is why the attached payload was `None` on
    /// every real turn while the per-call leg went on working from its session
    /// cache and reported the feature alive.
    ///
    /// Testing emptiness instead was the first attempt and it changed nothing
    /// live, precisely because a populated-but-reasoning-free frame is the
    /// shape that actually arrives. The items are all there one layer up:
    /// `response.output_item.done` delivers each completed item as it lands,
    /// which is the same source the per-call cache is built from — so
    /// whenever the cache has something to replay, so does this.
    ///
    /// A frame that DOES carry reasoning still wins outright, the same rule
    /// [`completed_response_from_sse`] follows for the answer text: this fills
    /// a gap and never overwrites a real payload.
    fn replay_source<'a>(&'a self, output: &'a [Value]) -> std::borrow::Cow<'a, [Value]> {
        let carries_reasoning = |items: &[Value]| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
        };
        if carries_reasoning(output) || !carries_reasoning(&self.streamed_output) {
            return std::borrow::Cow::Borrowed(output);
        }
        // A turn-final group is only legal in `input` directly before the
        // message it preceded, and the fold promotes reasoning to turn-final
        // only on seeing a `message` item. When text streamed as deltas
        // without a closing `message` frame, appending the marker is what lets
        // that turn's reasoning survive — and withholding it when no text
        // streamed is what keeps a reasoning-only `incomplete` turn from
        // claiming a position no message will occupy.
        let has_message = self
            .streamed_output
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some("message"));
        if has_message || !self.streamed_text {
            return std::borrow::Cow::Borrowed(&self.streamed_output);
        }
        let mut reconstructed = self.streamed_output.clone();
        reconstructed.push(json!({ "type": "message" }));
        std::borrow::Cow::Owned(reconstructed)
    }

    fn completed_output_deltas(&mut self, event: &Value) -> Vec<StreamEvent> {
        let Some(output) = event.pointer("/response/output").and_then(Value::as_array) else {
            // No `output` key at all still leaves the turn's streamed items,
            // and dropping them here would throw away the reasoning of every
            // turn whose terminal frame is shaped this way.
            let replay = self.replay_source(&[]).into_owned();
            self.reasoning_replay = reasoning_replay_from_output(&replay, &self.session_id);
            return Vec::new();
        };
        // Bound to a local so the borrow of `self` ends before the assignment
        // below takes `&mut self`.
        let mut events = Vec::new();
        for (index, item) in output.iter().enumerate() {
            let Ok(index) = u32::try_from(index) else {
                continue;
            };
            match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                "message" => events.extend(self.message_done_delta(index, item)),
                "function_call" => events.extend(self.function_call_done_delta(index, item)),
                _ => {}
            }
        }
        // Rebuilt from the whole turn rather than the incremental
        // `self.pending_reasoning` bucket `block_stop` maintains, so a
        // `function_call` whose individual `output_item.done` frame was
        // skipped or reordered is still covered (the same guarantee
        // `message_done_delta` / `function_call_done_delta` above rely on) —
        // reading whichever view of that turn is actually populated, since a
        // `store: false` backend leaves this frame's `output` empty.
        let replay = self.replay_source(output).into_owned();
        self.reasoning_replay = reasoning_replay_from_output(&replay, &self.session_id);
        events
    }
}

/// Map a `response.incomplete` close onto zo's Anthropic-shaped stop
/// reasons: an output-token cutoff is `max_tokens`; anything else (e.g. a
/// content filter) still ends the turn cleanly.
fn incomplete_stop_reason(event: &Value) -> &'static str {
    match event
        .pointer("/response/incomplete_details/reason")
        .and_then(Value::as_str)
    {
        Some("max_output_tokens") => "max_tokens",
        _ => "end_turn",
    }
}

const fn zero_usage() -> Usage {
    Usage {
        input_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        output_tokens: 0,
        output_tokens_details: None,
    }
}

fn output_index(event: &Value) -> Option<u32> {
    event
        .get("output_index")
        .and_then(Value::as_u64)
        .and_then(|index| u32::try_from(index).ok())
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn usage_field(usage: Option<&Value>, key: &str) -> u32 {
    usage
        .and_then(|value| value.get(key))
        .and_then(Value::as_u64)
        .and_then(|count| u32::try_from(count).ok())
        .unwrap_or(0)
}

/// Default cap on transparent mid-stream restarts. Seven bounded restarts give
/// a transient failure eight total attempts while [`MAX_RESTART_WALLCLOCK`]
/// still caps a slow or silent reconnect storm by elapsed time.
const DEFAULT_STREAM_MAX_RETRIES: u32 = 7;
const DEFAULT_STREAM_INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);
const DEFAULT_STREAM_MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

/// Total wall-clock ceiling over a single pre-commit restart sequence. The
/// per-attempt cap (`max_retries`) does not bound elapsed time, so a silent
/// backend that idle-times-out (`CHATGPT_STREAM_IDLE_TIMEOUT_MS`) and re-opens
/// each time can hold the turn for minutes before exhausting attempts (the
/// observed ~275 s freeze). Once the sequence has been retrying longer than
/// this, the next fault propagates as a retryable error instead of restarting
/// again, so the turn fails fast and the UI is freed. Sized above one idle
/// timeout plus a couple of brisk re-opens, below the multi-minute storm.
const MAX_RESTART_WALLCLOCK: std::time::Duration = std::time::Duration::from_secs(120);

/// Client for the ChatGPT subscription backend (Responses API). Built from a
/// stored ChatGPT OAuth token; the access token is sent as a bearer credential
/// and the `account_id` (from the `id_token` JWT) as the `chatgpt-account-id`
/// header.
/// The WebSocket connection kept from the last completed response, shared by
/// a client's clones: the next request that extends that response rides it as
/// a continuation (`previous_response_id`, only the new items) instead of
/// opening a connection and sending the whole conversation again.
type HeldSlot = std::sync::Arc<std::sync::Mutex<Option<websocket::HeldConnection>>>;

#[derive(Debug, Clone)]
pub struct ChatGptBackendClient {
    http: reqwest::Client,
    access_token: String,
    account_id: Option<String>,
    session_id: String,
    /// Stable cache scope for `prompt_cache_key` derivation and the
    /// reasoning-replay fallback cache — the zo session id when the host
    /// provides one. `session_id` above is deliberately NOT used for caching:
    /// it is a random per-client-instance value (Codex-parity wire header),
    /// and clients are rebuilt on every provider-route model swap (each
    /// smart-policy delegation episode) and every OAuth near-expiry rotation,
    /// so keying the provider cache on it rolled the key mid-session and
    /// pinned sol cache reads at the shared ~12k system prefix (observed live
    /// 07-20, hit-rate median 9%). `None` falls back to `session_id` (bare
    /// constructions in tests keep their legacy behavior).
    cache_scope: Option<String>,
    base_url: String,
    max_retries: u32,
    initial_backoff: std::time::Duration,
    max_backoff: std::time::Duration,
    /// An explicit transport for this client; `None` reads
    /// `ZO_CHATGPT_TRANSPORT` per request. Tests pin it here instead of the
    /// process env, which the rest of the suite reads without a lock.
    transport: Option<websocket::Transport>,
    held: HeldSlot,
}

impl ChatGptBackendClient {
    #[must_use]
    pub fn new(access_token: impl Into<String>, account_id: Option<String>) -> Self {
        Self {
            http: shared_http_client(),
            access_token: access_token.into(),
            account_id,
            session_id: random_session_id(),
            cache_scope: None,
            base_url: CHATGPT_RESPONSES_URL.to_string(),
            max_retries: DEFAULT_STREAM_MAX_RETRIES,
            initial_backoff: DEFAULT_STREAM_INITIAL_BACKOFF,
            max_backoff: DEFAULT_STREAM_MAX_BACKOFF,
            transport: None,
            held: HeldSlot::default(),
        }
    }

    /// Pin the prompt-cache scope (see [`Self::cache_scope`] field docs) to a
    /// host-stable id — the zo session id — so the provider cache key survives
    /// client rebuilds (model swaps, OAuth rotations, 401 recovery).
    #[must_use]
    pub fn with_cache_scope(mut self, scope: impl Into<String>) -> Self {
        let scope = scope.into();
        self.cache_scope = (!scope.is_empty()).then_some(scope);
        self
    }

    /// The active cache scope: the pinned host scope, else the per-instance
    /// wire session id.
    fn cache_scope(&self) -> &str {
        self.cache_scope.as_deref().unwrap_or(&self.session_id)
    }

    /// The pinned host cache scope, if any — used to carry the scope across
    /// client rebuilds (401 recovery reconstructs the client from scratch).
    #[must_use]
    pub fn pinned_cache_scope(&self) -> Option<&str> {
        self.cache_scope.as_deref()
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Tune the transparent mid-stream restart budget. `max_retries` bounds how
    /// many times a stalled/dropped stream is re-issued before any non-replay-safe
    /// output is surfaced; backoff between attempts grows exponentially from
    /// `initial_backoff`, capped at `max_backoff`, with per-thread jitter.
    #[must_use]
    pub fn with_retry_policy(
        mut self,
        max_retries: u32,
        initial_backoff: std::time::Duration,
        max_backoff: std::time::Duration,
    ) -> Self {
        self.max_retries = max_retries;
        self.initial_backoff = initial_backoff;
        self.max_backoff = max_backoff;
        self
    }

    /// Exponential backoff for restart `attempt` (1-based), capped at
    /// `max_backoff`. Mirrors the Anthropic client's schedule; the caller adds
    /// jitter via `retry_backoff::spread_backoff`.
    fn backoff_for_attempt(&self, attempt: u32) -> Result<std::time::Duration, ApiError> {
        super::backoff_for_attempt(attempt, self.initial_backoff, self.max_backoff)
    }

    /// Open a fresh streaming Responses connection for `request` (POST + header
    /// application + success check). Shared by the initial `stream_message` and
    /// every transparent mid-stream restart so they issue an identical request.
    /// Pin the transport for this client (tests, and a host that has its own
    /// setting). Unset, the process knob `ZO_CHATGPT_TRANSPORT` decides.
    #[must_use]
    pub fn with_transport(mut self, transport: websocket::Transport) -> Self {
        self.transport = Some(transport);
        self
    }

    fn transport_choice(&self) -> websocket::Transport {
        self.transport.unwrap_or_else(websocket::transport_choice)
    }

    /// Whether this request goes out on a WebSocket: forced by
    /// `ZO_CHATGPT_TRANSPORT=ws`, refused by `=sse`, and under `auto` taken
    /// for an https backend until a handshake fails in this process. Plain
    /// http backends (local mocks, proxies) stay on SSE.
    fn websocket_wanted(&self) -> bool {
        match self.transport_choice() {
            websocket::Transport::Websocket => true,
            websocket::Transport::Sse => false,
            websocket::Transport::Auto => {
                self.base_url.starts_with("https://")
                    && !WEBSOCKET_FALLBACK.load(std::sync::atomic::Ordering::Relaxed)
            }
        }
    }

    /// Open the response's transport: a WebSocket when wanted and reachable,
    /// else the SSE body. A failed handshake under `auto` is reported once and
    /// turns the rest of the process to SSE; under `ws` it is the error.
    /// Open the stream for `request`: the socket when it is wanted (riding the
    /// held connection as a continuation when `continue_held` and the request
    /// extends the response it completed), else the SSE response.
    async fn open_transport(
        &self,
        request: &MessageRequest,
        continue_held: bool,
    ) -> Result<ChatGptTransport, ApiError> {
        if self.websocket_wanted() {
            match self.open_websocket(request, continue_held).await {
                Ok(socket) => return Ok(ChatGptTransport::Ws(Box::new(socket))),
                Err(error) if self.transport_choice() == websocket::Transport::Websocket => {
                    return Err(error);
                }
                Err(error) => {
                    WEBSOCKET_FALLBACK.store(true, std::sync::atomic::Ordering::Relaxed);
                    eprintln!(
                        "[zo] chatgpt websocket unavailable ({error}); using the SSE stream for the rest of this session"
                    );
                }
            }
        }
        let response = self.open_stream_response(request).await?;
        Ok(ChatGptTransport::Sse {
            response,
            parser: ResponsesSseParser::new(),
        })
    }

    /// Connect the socket and send the request, within the same open budget
    /// the HTTP path has.
    async fn open_websocket(
        &self,
        request: &MessageRequest,
        continue_held: bool,
    ) -> Result<websocket::ResponsesWebsocket, ApiError> {
        let open = self.open_websocket_unbounded(request, continue_held);
        let Some((budget, kind)) = stream_open_timeout() else {
            return open.await;
        };
        match tokio::time::timeout(budget, open).await {
            Ok(result) => result,
            Err(_) => Err(match kind {
                StreamOpenTimeoutKind::Idle => ApiError::stream_idle_timeout(budget),
                StreamOpenTimeoutKind::Startup => {
                    ApiError::stream_startup_no_progress(budget, false)
                }
            }),
        }
    }

    async fn open_websocket_unbounded(
        &self,
        request: &MessageRequest,
        continue_held: bool,
    ) -> Result<websocket::ResponsesWebsocket, ApiError> {
        let instructions = Self::instructions(request);
        let mut body =
            build_responses_request_for_session(request, &instructions, true, self.cache_scope());
        debug_dump_chatgpt("request", &body.to_string());
        if continue_held {
            if let Some(held) = self.take_held() {
                match held.continue_with(body).await {
                    Ok(socket) => {
                        debug_dump_chatgpt("transport", "websocket-continued");
                        return Ok(socket);
                    }
                    Err(returned) => body = returned,
                }
            }
        }
        let url = websocket::websocket_url(&self.base_url).ok_or_else(|| ApiError::StreamApi {
            error_type: Some("websocket_handshake".to_string()),
            message: Some(format!("no websocket form for {}", self.base_url)),
            body: String::new(),
            retryable: false,
        })?;
        let mut headers = vec![
            ("OpenAI-Beta", websocket::OPENAI_BETA_RESPONSES_WEBSOCKETS.to_string()),
            ("originator", ORIGINATOR.to_string()),
            ("user-agent", USER_AGENT.to_string()),
            ("session_id", self.session_id.clone()),
            ("authorization", format!("Bearer {}", self.access_token)),
        ];
        if let Some(account_id) = &self.account_id {
            headers.push(("chatgpt-account-id", account_id.clone()));
        }
        let mut socket = websocket::ResponsesWebsocket::connect(&url, &headers).await?;
        debug_dump_chatgpt("transport", "websocket");
        socket.send_create(body).await?;
        Ok(socket)
    }

    fn take_held(&self) -> Option<websocket::HeldConnection> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn keep_held(&self, connection: websocket::HeldConnection) {
        *self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(connection);
    }

    async fn open_stream_response(
        &self,
        request: &MessageRequest,
    ) -> Result<reqwest::Response, ApiError> {
        let open = self.open_stream_response_unbounded(request);
        let Some((budget, kind)) = stream_open_timeout() else {
            return open.await;
        };
        match tokio::time::timeout(budget, open).await {
            Ok(result) => result,
            Err(_) => Err(match kind {
                StreamOpenTimeoutKind::Idle => ApiError::stream_idle_timeout(budget),
                StreamOpenTimeoutKind::Startup => {
                    ApiError::stream_startup_no_progress(budget, false)
                }
            }),
        }
    }

    /// Raw POST + response-header/body validation. The public stream opener
    /// wraps this whole future in the same idle/startup budget used after SSE
    /// headers arrive; otherwise a backend that accepts the socket but never
    /// answers the HTTP request bypasses every body-level watchdog.
    async fn open_stream_response_unbounded(
        &self,
        request: &MessageRequest,
    ) -> Result<reqwest::Response, ApiError> {
        let instructions = Self::instructions(request);
        let body =
            build_responses_request_for_session(request, &instructions, true, self.cache_scope());
        debug_dump_chatgpt("request", &body.to_string());
        let response = self
            .apply_headers(self.http.post(&self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(ApiError::from)?;
        expect_success(response).await
    }

    /// zo's system blocks become the Responses `instructions` field — the
    /// Responses API takes the system role at the top level, not as an `input`
    /// item.
    fn instructions(request: &MessageRequest) -> String {
        let system = request.system.as_ref().map_or_else(String::new, |blocks| {
            blocks
                .iter()
                .map(|block| match block {
                    SystemBlock::Text { text, .. } => text.as_str(),
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
        if system.is_empty() {
            return system;
        }
        // zo's base prompt is authored for Claude Code and hardcodes a Claude
        // identity ("You are Claude Code…"). For an
        // OpenAI-served model that text makes it introduce itself as Claude, so
        // prepend an explicit identity override. The tooling / workflow guidance
        // below still applies; only the identity is corrected. Routed through the
        // shared `apply_non_anthropic_identity` so every non-Anthropic backend
        // corrects the identity identically (Gemini / OpenAI-compatible too).
        let (model, _) = chatgpt_model_and_speed(&request.model);
        let identity = super::apply_non_anthropic_identity(
            &system,
            &model,
            super::maker_for_provider(super::ProviderKind::OpenAi),
        );
        // The base prompt's parallel-tools prose alone leaves GPT models
        // averaging ~2.6 tool calls per tool-using message (live sessions
        // 07-19/20) versus ~3.5 for Claude on the same harness — each skipped
        // batch is a whole extra round trip that re-bills the transcript, so
        // the backend restates the contract in the imperative register GPT
        // models follow best, as a trailing section (both prompt ends carry
        // instruction weight; the head must stay identity-first).
        format!("{identity}{TOOL_BATCHING_CONTRACT}")
    }

    fn apply_headers(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut builder = builder
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("OpenAI-Beta", OPENAI_BETA_RESPONSES)
            .header("originator", ORIGINATOR)
            // Model-gating fingerprint (see [`USER_AGENT`]): without it the
            // backend 404s `gpt-5.6-luna` as "Model not found".
            .header("user-agent", USER_AGENT)
            .header("session_id", &self.session_id)
            .bearer_auth(&self.access_token);
        if let Some(account_id) = &self.account_id {
            builder = builder.header("chatgpt-account-id", account_id);
        }
        builder
    }

    pub async fn generate_image(
        &self,
        model: &str,
        prompt: &str,
        size: Option<&str>,
        quality: Option<&str>,
    ) -> Result<String, ApiError> {
        let body = build_image_generation_request(model, prompt, size, quality);
        let response = self
            .apply_headers(self.http.post(&self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(ApiError::from)?;
        let idle_timeout = super::stream_idle_timeout();
        let response = match idle_timeout {
            Some(idle) => tokio::time::timeout(idle, expect_success(response))
                .await
                .map_err(|_| ApiError::stream_idle_timeout(idle))??,
            None => expect_success(response).await?,
        };
        let body = read_image_generation_sse(response, idle_timeout).await?;
        image_generation_result_from_sse(&body)
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<ChatGptStream, ApiError> {
        let startup_window = startup_no_progress_timeout();
        let startup_started_at = std::time::Instant::now();
        let transport = self.open_transport(request, true).await?;
        Ok(ChatGptStream {
            transport,
            state: ResponsesStreamState::new(request.model.clone())
                .with_session_id(self.cache_scope().to_string()),
            pending: VecDeque::new(),
            done: false,
            // Retry context: the stream re-issues this exact request through
            // `client` while it is still re-armable (no text/tool args surfaced).
            client: self.clone(),
            request: request.clone(),
            restart_attempts: 0,
            restart_window_start: None,
            committed: false,
            retry_notice: None,
            startup_window,
            startup_deadline: startup_window
                .and_then(|window| startup_started_at.checked_add(window)),
            startup_reasoning_extended: false,
        })
    }

    /// One-shot (non-streaming) turn.
    ///
    /// The Codex Responses endpoint has no non-streaming mode: a `stream: false`
    /// body is rejected outright with `400 {"detail":"Stream must be set to
    /// true"}`. So this issues a STREAMED request and folds the SSE body back
    /// into a single response — the caller's non-streaming contract is
    /// preserved, and `parse_responses_response` still does the parsing because
    /// `response.completed` carries the same complete response object the
    /// one-shot endpoint would have returned.
    ///
    /// Before this, every `send_message` caller on a ChatGPT-subscription model
    /// failed 100% of the time. The routing probe was the load-bearing one: it
    /// fails open, so `/smart classifier probed` looked enabled while never
    /// once producing an assessment.
    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        let instructions = Self::instructions(request);
        let body =
            build_responses_request_for_session(request, &instructions, true, self.cache_scope());
        let response = self
            .apply_headers(self.http.post(&self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(ApiError::from)?;
        let response = expect_success(response).await?;
        let body = response.text().await.map_err(ApiError::from)?;
        let value = completed_response_from_sse(&body).ok_or(ApiError::InvalidSseFrame(
            "Codex stream ended without a response.completed event",
        ))?;
        Ok(parse_responses_response(
            &value,
            &request.model,
            self.cache_scope(),
        ))
    }
}

const MAX_IMAGE_GENERATION_SSE_BYTES: usize = 64 * 1024 * 1024;

async fn read_image_generation_sse(
    mut response: reqwest::Response,
    idle_timeout: Option<std::time::Duration>,
) -> Result<String, ApiError> {
    let mut body = Vec::new();
    loop {
        let chunk = match idle_timeout {
            Some(idle) => tokio::time::timeout(idle, response.chunk())
                .await
                .map_err(|_| ApiError::stream_idle_timeout(idle))??,
            None => response.chunk().await?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > MAX_IMAGE_GENERATION_SSE_BYTES)
        {
            return Err(ApiError::InvalidSseFrame(
                "Codex image generation response exceeded the safe size limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| {
        ApiError::InvalidSseFrame("Codex image generation returned non-UTF-8 SSE data")
    })
}

fn image_generation_result_from_sse(body: &str) -> Result<String, ApiError> {
    let mut latest_partial = None;
    let mut final_result = None;
    let mut completed = false;
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("response.image_generation_call.partial_image") => {
                if let Some(result) = event.get("partial_image_b64").and_then(Value::as_str) {
                    latest_partial = Some(result.to_string());
                }
            }
            Some("response.image_generation_call.completed") => {
                if let Some(result) = event.get("result").and_then(Value::as_str) {
                    final_result = Some(result.to_string());
                }
            }
            Some("response.output_item.done") => {
                if let Some(result) = event
                    .get("item")
                    .filter(|item| item.get("type").and_then(Value::as_str) == Some("image_generation_call"))
                    .and_then(|item| item.get("result"))
                    .and_then(Value::as_str)
                {
                    final_result = Some(result.to_string());
                }
            }
            Some("response.completed") => {
                completed = true;
                if let Some(result) = event
                    .get("response")
                    .and_then(|response| response.get("output"))
                    .and_then(Value::as_array)
                    .and_then(|items| {
                        items.iter().rev().find_map(|item| {
                            (item.get("type").and_then(Value::as_str)
                                == Some("image_generation_call"))
                            .then(|| item.get("result").and_then(Value::as_str))
                            .flatten()
                        })
                    })
                {
                    final_result = Some(result.to_string());
                }
            }
            Some("response.failed" | "response.incomplete" | "error") => {
                return Err(image_generation_failure(&event));
            }
            _ => {}
        }
    }
    if !completed {
        return Err(ApiError::InvalidSseFrame(
            "Codex image generation stream ended before response.completed",
        ));
    }
    // With `store: false`, the subscription backend can leave terminal output
    // empty and place the completed PNG in the last partial-image event.
    final_result.or(latest_partial).ok_or(ApiError::InvalidSseFrame(
        "Codex image generation completed without image data",
    ))
}

fn image_generation_failure(event: &Value) -> ApiError {
    let event_type = event.get("type").and_then(Value::as_str).unwrap_or_default();
    let (code, message) = match event_type {
        "response.failed" => (
            event.pointer("/response/error/code").and_then(Value::as_str),
            event.pointer("/response/error/message").and_then(Value::as_str),
        ),
        "response.incomplete" => (
            Some("response_incomplete"),
            event
                .pointer("/response/incomplete_details/reason")
                .and_then(Value::as_str),
        ),
        _ => (
            event.get("code").and_then(Value::as_str),
            event.get("message").and_then(Value::as_str),
        ),
    };
    let code = code.unwrap_or(event_type);
    let retryable = matches!(
        code,
        "server_error" | "rate_limit_exceeded" | "overloaded" | "slow_down"
    );
    ApiError::StreamApi {
        error_type: (!code.is_empty()).then(|| code.to_string()),
        message: message.map(str::to_string).or_else(|| {
            failure_frame_digest(event)
                .map(|digest| format!("image generation failed ({digest})"))
        }),
        body: String::new(),
        retryable,
    }
}

/// Fold a drained Responses SSE body back into the single response object the
/// one-shot endpoint would have returned.
///
/// Two things have to happen here, and the second is the non-obvious one:
///
/// 1. Take the terminal frame's `response` — `response.completed` for a
///    finished turn, `response.incomplete` for one cut short by a token cap.
///    A truncated answer is still an answer, so the last of either wins.
/// 2. **Rebuild `output` from the text deltas.** Codex runs with `store:
///    false`, and its terminal frame carries `"output": []` — the answer exists
///    only in the `response.output_text.delta` events. Returning the terminal
///    frame verbatim therefore yields a well-formed response whose content is
///    empty, which is exactly what made the routing probe parse every reply as
///    malformed and fall back, silently, on every single call.
///
/// A terminal frame that *does* carry `output` is left untouched, so this only
/// fills a gap and never overwrites a real payload. Non-JSON frames,
/// keep-alives, and `[DONE]` are skipped.
fn completed_response_from_sse(body: &str) -> Option<Value> {
    let mut latest: Option<Value> = None;
    let mut streamed_text = String::new();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    streamed_text.push_str(delta);
                }
            }
            Some("response.completed" | "response.incomplete") => {
                if let Some(response) = event.get("response") {
                    latest = Some(response.clone());
                }
            }
            _ => {}
        }
    }
    let mut response = latest?;
    let output_missing = response
        .get("output")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    if output_missing && !streamed_text.is_empty() {
        response["output"] = serde_json::json!([{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": streamed_text }],
        }]);
    }
    Some(response)
}

/// Streamed Responses turn: pulls chunks, feeds the SSE parser, and yields
/// zo [`StreamEvent`]s as content blocks complete.
///
/// While no non-replay-safe output has been surfaced yet (`committed == false`),
/// a stalled or dropped stream is transparently re-issued via `client` — the
/// common "switched to gpt-5.5, it reasons silently, connection idles out before
/// the first answer/tool token" case recovers without bubbling an error.
/// Provider bookkeeping frames and reasoning summaries are safe to replay; text
/// deltas and tool-call argument deltas are not. The Codex backend forces
/// `store: false`, so there is no server-side cursor to resume from — a restart
/// re-runs the whole turn, which is only safe before the commit point.
/// Once a WebSocket handshake fails under `auto`, the rest of this process
/// speaks SSE: one failed attempt per session, not one per request (codex's
/// `disable_websockets` has the same scope).
static WEBSOCKET_FALLBACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How the response reaches this stream: the SSE body of an HTTP response, or
/// frames on a WebSocket. Both hand the loop the same JSON events; only the
/// framing differs, so everything after `next_batch` is shared.
enum ChatGptTransport {
    Sse {
        response: reqwest::Response,
        parser: ResponsesSseParser,
    },
    Ws(Box<websocket::ResponsesWebsocket>),
    /// The response ended; there is nothing more to read.
    Done,
}

impl std::fmt::Debug for ChatGptTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sse { response, parser } => f
                .debug_struct("Sse")
                .field("response", response)
                .field("parser", parser)
                .finish(),
            Self::Ws(socket) => f.debug_tuple("Ws").field(socket).finish(),
            Self::Done => f.write_str("Done"),
        }
    }
}

impl ChatGptTransport {
    /// The next chunk's events: `Some(vec![])` for a keepalive (an SSE comment
    /// line, a WebSocket ping), `None` at the end of the body or the socket.
    async fn next_batch(&mut self) -> Result<Option<Vec<Value>>, ApiError> {
        match self {
            Self::Sse { response, parser } => match response.chunk().await.map_err(ApiError::from)? {
                Some(chunk) => Ok(Some(parser.push(&chunk)?)),
                None => Ok(None),
            },
            Self::Ws(socket) => socket.next_batch().await,
            Self::Done => Ok(None),
        }
    }

    /// Whether a response's terminal event ends this stream on its own: a
    /// WebSocket stays open for the next request, so the terminal event is the
    /// end; an SSE body ends when the server closes it.
    fn ends_at_terminal_event(&self) -> bool {
        matches!(self, Self::Ws(_))
    }

    /// The response is over. A socket whose response completed is kept by the
    /// client for the next request; anything else is closed.
    async fn finish(&mut self, client: &ChatGptBackendClient) {
        if !matches!(self, Self::Ws(_)) {
            return;
        }
        if let Self::Ws(socket) = std::mem::replace(self, Self::Done) {
            if let Some(connection) = (*socket).finish().await {
                client.keep_held(connection);
            }
        }
    }
}

pub struct ChatGptStream {
    transport: ChatGptTransport,
    state: ResponsesStreamState,
    pending: VecDeque<StreamEvent>,
    done: bool,
    /// Client + request used to re-open the stream on a pre-commit fault.
    client: ChatGptBackendClient,
    request: MessageRequest,
    /// Transparent restarts spent so far, bounded by `client.max_retries`.
    restart_attempts: u32,
    /// Wall clock at the first restart of the current pre-commit sequence, so the
    /// whole restart storm is bounded by elapsed time and not only by attempt
    /// count — a silent backend that idle-times-out and re-opens repeatedly would
    /// otherwise hold the turn for minutes (see [`MAX_RESTART_WALLCLOCK`]).
    restart_window_start: Option<std::time::Instant>,
    /// Set once text or tool-call argument bytes are surfaced; locks out further
    /// restarts to avoid duplicate user-visible output or malformed tools.
    committed: bool,
    /// Optional sink invoked just before each transparent restart sleeps, so a
    /// live UI can surface the otherwise-silent multi-second reconnect pause as
    /// "reconnecting" instead of a freeze. `None` (the default) preserves the
    /// old log-only behaviour for non-interactive callers.
    retry_notice: Option<StreamRetryCallback>,
    /// First-action watchdog for keep-alive-only streams. Transport bytes do not
    /// count as progress; one decoded reasoning delta grants one extra window.
    startup_window: Option<std::time::Duration>,
    startup_deadline: Option<std::time::Instant>,
    startup_reasoning_extended: bool,
}

/// Sink for [`ChatGptStream`] mid-stream restart notices. Boxed `Fn` so the
/// runtime can route it to a render channel without `api` depending on any
/// render type.
type StreamRetryCallback = std::sync::Arc<dyn Fn(StreamRetryNotice) + Send + Sync>;

impl std::fmt::Debug for ChatGptStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatGptStream")
            .field("transport", &self.transport)
            .field("state", &self.state)
            .field("pending", &self.pending)
            .field("done", &self.done)
            .field("client", &self.client)
            .field("request", &self.request)
            .field("restart_attempts", &self.restart_attempts)
            .field("restart_window_start", &self.restart_window_start)
            .field("committed", &self.committed)
            .field("has_retry_notice", &self.retry_notice.is_some())
            .field("startup_window", &self.startup_window)
            .field("startup_deadline", &self.startup_deadline)
            .field(
                "startup_reasoning_extended",
                &self.startup_reasoning_extended,
            )

            .finish()
    }
}

const CHATGPT_STREAM_COOPERATIVE_YIELD_EVERY: usize = 16;

/// How long the stream may stay quiet (keep-alive chunks arriving, no decoded
/// event) before the one-shot quiet-reasoning heartbeat notice fires. Kept just
/// under the CLI's 20-second "no output" stall-badge threshold
/// (`STALL_THRESHOLD_SECS`) so that, whenever keep-alives are still arriving,
/// the badge latches to the calm "reasoning · stream alive" *before* it would
/// ever read "no output" — the whole point of the heartbeat. At the old 60s it
/// lost that race by 40 seconds, so a healthy gpt-5.x reasoning pass showed the
/// alarming "no output" for most of a minute (live report: users could not tell
/// a reasoning turn from a hang). Still above any normal first-token gap — a
/// real delta resets `quiet_since`, so fast turns never trip it — and firing the
/// notice only pre-arms the badge; nothing is shown until the 20s threshold, so
/// an earlier fire adds no visual noise. A dead/congested connection sends no
/// chunks at all, so this branch never runs and the idle/startup timeouts own
/// that case instead.
const CHATGPT_QUIET_REASONING_NOTICE_AFTER: std::time::Duration =
    std::time::Duration::from_secs(15);

/// Default idle budget: abort a chunk read that has received no bytes for this
/// long. The ChatGPT/Codex Responses backend keeps the HTTP/2 connection alive
/// while it reasons silently, so `chunk().await` can otherwise block forever.
/// Sized well above any normal inter-chunk gap so legitimate slow reasoning is
/// not cut short.
const CHATGPT_STREAM_IDLE_TIMEOUT_MS: u64 = 90_000;

/// Override for [`CHATGPT_STREAM_IDLE_TIMEOUT_MS`]. A value of `0` disables the
/// idle timeout entirely (restores the unbounded-wait behaviour).
const CHATGPT_STREAM_IDLE_TIMEOUT_ENV: &str = "ZO_CHATGPT_STREAM_IDLE_TIMEOUT_MS";

/// First decoded task action deadline. Unlike the 90-second byte-idle guard,
/// this clock is not reset by transport keep-alives. A reasoning delta grants
/// one equal extension, matching the effort-aware workflow startup watchdog.
const CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_MS: u64 = 240_000;
const CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_ENV: &str =
    "ZO_CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_MS";

/// Resolve the per-chunk idle budget, honouring the env override. `None` means
/// "no timeout" (override set to `0`).
fn stream_idle_timeout() -> Option<std::time::Duration> {
    let millis = std::env::var(CHATGPT_STREAM_IDLE_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(CHATGPT_STREAM_IDLE_TIMEOUT_MS);
    (millis > 0).then(|| std::time::Duration::from_millis(millis))
}

fn startup_no_progress_timeout() -> Option<std::time::Duration> {
    let millis = std::env::var(CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_MS);
    (millis > 0).then(|| std::time::Duration::from_millis(millis))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamOpenTimeoutKind {
    Idle,
    Startup,
}

/// Bound the HTTP response-open stage as well as the SSE body. The byte-idle
/// window normally wins (90s vs. the 4m first-action window); disabling it
/// still leaves the startup deadline as a safe upper bound.
fn stream_open_timeout() -> Option<(std::time::Duration, StreamOpenTimeoutKind)> {
    match (stream_idle_timeout(), startup_no_progress_timeout()) {
        (Some(idle), Some(startup)) if idle <= startup => {
            Some((idle, StreamOpenTimeoutKind::Idle))
        }
        (Some(_) | None, Some(startup)) => {
            Some((startup, StreamOpenTimeoutKind::Startup))
        }
        (Some(idle), None) => Some((idle, StreamOpenTimeoutKind::Idle)),
        (None, None) => None,
    }
}

fn extend_startup_deadline_for_reasoning(
    deadline: &mut Option<std::time::Instant>,
    window: Option<std::time::Duration>,
    already_extended: &mut bool,
) {
    if *already_extended {
        return;
    }
    if let (Some(current), Some(extension)) = (*deadline, window) {
        *deadline = current.checked_add(extension);
        *already_extended = true;
    }
}

use super::{crosses_restart_commit_boundary, should_restart_within_budget};

impl ChatGptStream {
    /// Install a sink that fires just before each transparent restart sleeps, so
    /// a live consumer can render the reconnect pause. Chainable; no-op sink by
    /// default.
    #[must_use]
    pub fn with_retry_notice_callback(
        mut self,
        callback: impl Fn(StreamRetryNotice) + Send + Sync + 'static,
    ) -> Self {
        self.retry_notice = Some(std::sync::Arc::new(callback));
        self
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        let idle_timeout = stream_idle_timeout();
        let mut quiet_chunks = 0usize;
        // Quiet-reasoning heartbeat: the connection is delivering keep-alive
        // chunks (so the idle timeout never fires) but the model has emitted
        // no event yet — deep reasoning on a large context can stay silent
        // for minutes, and without a signal the live UI reads "no output Nm"
        // as a hang. Fired once per quiet stretch, through the same sink the
        // reconnect notices ride.
        let mut quiet_since: Option<std::time::Instant> = None;
        let mut quiet_notified = false;
        loop {
            if let Some(event) = self.pending.pop_front() {
                self.observe_startup_event(&event);
                // Reasoning may extend the deadline once, but it must not keep
                // an event-producing stream alive forever. Check after observing
                // the event so the first reasoning delta receives its extension,
                // and before surfacing it so an already-expired stream remains
                // replay-safe when we restart it.
                if self.restart_if_startup_deadline_elapsed().await? {
                    quiet_chunks = 0;
                    quiet_since = None;
                    quiet_notified = false;
                    continue;
                }
                return Ok(Some(event));
            }
            // A terminal `response.failed` / `error` frame surfaces as a real
            // error (restartable pre-commit) — never as a silent zero-event
            // end-of-stream the runtime would misread as an empty assistant
            // turn. It is handled only once the queue above is drained: one
            // chunk can carry visible text AND the terminal frame, and that
            // text is valid output the caller must still receive. Draining
            // first also means `observe_startup_event` has already set the
            // commit boundary for it, so the replay decision below sees a
            // committed turn without a second mechanism to keep in sync.
            if let Some(failure) = self.state.take_failure() {
                self.recover_or_restart_precommit(failure).await?;
                quiet_chunks = 0;
                quiet_since = None;
                quiet_notified = false;
                continue;
            }
            if self.done {
                return Ok(None);
            }
            if self.restart_if_startup_deadline_elapsed().await? {
                quiet_chunks = 0;
                quiet_since = None;
                quiet_notified = false;
                continue;
            }
            // Per-chunk idle timeout: each received chunk resets the budget, so
            // long-but-active streams are never cut, while a truly silent
            // backend (Codex holds the HTTP/2 connection open while it reasons)
            // surfaces a retryable error instead of hanging the turn forever.
            // The startup deadline participates in this wait too: without its
            // own wake-up, a final keepalive just before the deadline could leave
            // `chunk().await` asleep until the longer transport-idle timer.
            let chunk = match self.read_batch(idle_timeout).await {
                Ok(chunk) => chunk,
                Err(error) => {
                    if self.reopen_if_continuation_failed(Some(&error)).await? {
                        quiet_since = None;
                        quiet_notified = false;
                    } else {
                        self.recover_or_restart_precommit(error).await?;
                    }
                    quiet_chunks = 0;
                    continue;
                }
            };
            match chunk {
                Some(batch) => {
                    let emitted = self.ingest_batch(batch).await;
                    if emitted {
                        quiet_chunks = 0;
                        quiet_since = None;
                        quiet_notified = false;
                    } else {
                        quiet_chunks = quiet_chunks.saturating_add(1);
                        if quiet_chunks >= CHATGPT_STREAM_COOPERATIVE_YIELD_EVERY {
                            quiet_chunks = 0;
                            tokio::task::yield_now().await;
                        }
                        let since = *quiet_since.get_or_insert_with(std::time::Instant::now);
                        if let Some(error) = self.startup_no_progress_error() {
                            if self.can_restart(&error) {
                                self.restart(error).await?;
                                quiet_chunks = 0;
                                quiet_since = None;
                                quiet_notified = false;
                                continue;
                            }
                            return Err(self.wrap_restart_exhaustion(error));
                        }
                        if !quiet_notified
                            && since.elapsed() >= CHATGPT_QUIET_REASONING_NOTICE_AFTER
                        {
                            quiet_notified = true;
                            if let Some(notice) = &self.retry_notice {
                                notice(StreamRetryNotice {
                                    kind: core_types::StreamNoticeKind::QuietReasoning,
                                    label: core_types::QUIET_REASONING_LABEL,
                                    attempt: 0,
                                    max_attempts: 0,
                                    delay: since.elapsed(),
                                });
                            }
                        }
                    }
                }
                None => {
                    if self.reopen_if_continuation_failed(None).await? {
                        quiet_chunks = 0;
                        quiet_since = None;
                        quiet_notified = false;
                    } else {
                        self.done = true;
                    }
                }
            }
        }
    }

    /// The next batch from the transport, within the wait the watchdogs allow.
    async fn read_batch(
        &mut self,
        idle_timeout: Option<std::time::Duration>,
    ) -> Result<Option<Vec<Value>>, ApiError> {
        match self.next_chunk_wait_budget(idle_timeout) {
            Some(wait) => match tokio::time::timeout(wait, self.transport.next_batch()).await {
                Ok(batch) => batch,
                Err(_elapsed) => Err(self.startup_no_progress_error().unwrap_or_else(|| {
                    ApiError::stream_idle_timeout(idle_timeout.unwrap_or(wait))
                })),
            },
            None => self.transport.next_batch().await,
        }
    }

    /// A request that rode a held connection as a continuation may find the
    /// socket gone (nobody answered the server's pings between requests) or
    /// the previous response gone from the server
    /// (`previous_response_not_found` — codex's "Previous response was not
    /// found. Retrying the full request."). Neither is the provider failing:
    /// the full request goes out on a fresh connection at once, with no retry
    /// counted and no backoff. At most once per request: the connection this
    /// opens is fresh, and a fresh connection is never a continuation.
    async fn reopen_if_continuation_failed(
        &mut self,
        error: Option<&ApiError>,
    ) -> Result<bool, ApiError> {
        let ChatGptTransport::Ws(socket) = &self.transport else {
            return Ok(false);
        };
        if !socket.sent_incrementally() {
            return Ok(false);
        }
        let gone = error.is_some_and(websocket::is_previous_response_not_found)
            || !socket.has_progressed();
        if !gone {
            return Ok(false);
        }
        debug_dump_chatgpt("transport", "websocket-continuation-refused");
        self.transport = self.client.open_transport(&self.request, false).await?;
        self.state = ResponsesStreamState::new(self.request.model.clone())
            .with_session_id(self.client.cache_scope().to_string());
        self.pending.clear();
        self.done = false;
        self.reset_startup_watchdog();
        Ok(true)
    }

    /// Fold one transport batch into the event queue; `true` when it carried
    /// at least one event (a keepalive carries none). On a socket the
    /// response's terminal event is where this stream ends — the connection
    /// stays open for the next request and would otherwise be read forever.
    async fn ingest_batch(&mut self, batch: Vec<Value>) -> bool {
        let mut emitted = false;
        for value in batch {
            let events = self.state.ingest(&value);
            emitted |= !events.is_empty();
            self.pending.extend(events);
            if self.transport.ends_at_terminal_event() && websocket::is_terminal_event(&value) {
                self.done = true;
                self.transport.finish(&self.client).await;
            }
        }
        emitted
    }

    /// Recover a replay-safe stream failure or transparently reopen the stream.
    /// Returning `Ok` means the caller should resume its event loop; an unsafe
    /// or exhausted failure is returned unchanged.
    async fn recover_or_restart_precommit(&mut self, error: ApiError) -> Result<(), ApiError> {
        if self.recover_precommit_failure(&error).await? {
            return Ok(());
        }
        if self.can_restart(&error) {
            self.restart(error).await?;
            return Ok(());
        }
        // The ladder is spent and every attempt re-sent the same bytes. Changing
        // the request is the only lever left, so try shedding the image bulk
        // once before giving the turn up.
        if is_terminal_stream_failure(&error) && self.recover_by_shedding_images().await? {
            return Ok(());
        }
        Err(self.wrap_restart_exhaustion(error))
    }

    fn observe_startup_event(&mut self, event: &StreamEvent) {
        if crosses_restart_commit_boundary(event) {
            self.committed = true;
            self.startup_deadline = None;
            return;
        }
        let reasoning = matches!(
            event,
            StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                delta: ContentBlockDelta::ThinkingDelta { .. },
                ..
            })
        );
        if reasoning && !self.startup_reasoning_extended {
            extend_startup_deadline_for_reasoning(
                &mut self.startup_deadline,
                self.startup_window,
                &mut self.startup_reasoning_extended,
            );
        }
    }

    fn startup_no_progress_error(&self) -> Option<ApiError> {
        let deadline = self.startup_deadline?;
        if std::time::Instant::now() < deadline {
            return None;
        }
        let window = self.startup_window?;
        let budget = if self.startup_reasoning_extended {
            window.checked_add(window).unwrap_or(std::time::Duration::MAX)
        } else {
            window
        };
        Some(ApiError::stream_startup_no_progress(
            budget,
            self.startup_reasoning_extended,
        ))
    }

    fn next_chunk_wait_budget(
        &self,
        idle_timeout: Option<std::time::Duration>,
    ) -> Option<std::time::Duration> {
        let startup_remaining = self
            .startup_deadline
            .map(|deadline| deadline.saturating_duration_since(std::time::Instant::now()));
        match (idle_timeout, startup_remaining) {
            (Some(idle), Some(startup)) => Some(idle.min(startup)),
            (Some(idle), None) => Some(idle),
            (None, Some(startup)) => Some(startup),
            (None, None) => None,
        }
    }

    async fn restart_if_startup_deadline_elapsed(&mut self) -> Result<bool, ApiError> {
        let Some(error) = self.startup_no_progress_error() else {
            return Ok(false);
        };
        if self.can_restart(&error) {
            self.restart(error).await?;
            return Ok(true);
        }
        Err(self.wrap_restart_exhaustion(error))
    }

    fn reset_startup_watchdog(&mut self) {
        self.startup_deadline = self
            .startup_window
            .and_then(|window| std::time::Instant::now().checked_add(window));
        self.startup_reasoning_extended = false;
    }

    /// Last-resort recovery for a pre-commit terminal Responses stream failure.
    ///
    /// Once visible text or tool-call bytes have crossed the commit boundary we
    /// must not replay the request here: duplicate output or partial tool JSON is
    /// worse than a surfaced error. Before that boundary, however, the failed
    /// stream has produced no user-visible assistant content, so a single
    /// non-streaming retry with a lower reasoning tier can turn the common
    /// `xhigh` terminal stream failure into a normal completed response.
    async fn recover_precommit_failure(&mut self, error: &ApiError) -> Result<bool, ApiError> {
        if self.committed || !is_terminal_stream_failure(error) {
            return Ok(false);
        }
        let request = deescalated_recovery_request(&self.request);
        match self.client.send_message(&request).await {
            Ok(response) => {
                self.pending.extend(complete_response_events(response));
                self.done = true;
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    fn can_restart(&self, error: &ApiError) -> bool {
        should_restart_within_budget(
            self.committed,
            error.is_retryable(),
            self.restart_attempts,
            self.client.max_retries,
            self.restart_window_start.map(|start| start.elapsed()),
            MAX_RESTART_WALLCLOCK,
        )
    }

    fn wrap_restart_exhaustion(&self, error: ApiError) -> ApiError {
        let attempts_spent = self.restart_attempts >= self.client.max_retries;
        let wallclock_spent = self
            .restart_window_start
            .is_some_and(|start| start.elapsed() >= MAX_RESTART_WALLCLOCK);
        if !self.committed && error.is_retryable() && (attempts_spent || wallclock_spent) {
            ApiError::RetriesExhausted {
                attempts: self.restart_attempts.saturating_add(1),
                last_error: Box::new(error),
            }
        } else {
            error
        }
    }

    /// Last resort before a dead turn: re-open the stream once with the image
    /// bulk stripped out of the request.
    ///
    /// Every attempt the ladder just spent re-sent the *identical* bytes — both
    /// `restart` and [`deescalated_recovery_request`] change only the reasoning
    /// tier — so exhaustion proves replay cannot help. This is the one move that
    /// changes what is actually sent. It fires only when the images dominate the
    /// payload ([`shed_image_payload`]) and stays pre-commit (a committed turn
    /// must never replay), so a failure that had nothing to do with size costs
    /// one extra request and then surfaces the original error unchanged.
    ///
    /// It is inherently once-per-stream, with no flag to keep in sync: a shed
    /// request carries placeholders instead of pixels, so the very next
    /// [`shed_image_payload`] on it finds nothing left to shed and declines.
    ///
    /// Dropping the re-attached image items shortens the `input` array, so this
    /// request misses the prompt cache and re-bills the whole prefix. That is the
    /// price of the last attempt before a dead turn, and it is why the gate is
    /// deliberately narrow rather than opportunistic.
    ///
    /// Returns `true` when the caller should resume its event loop on the
    /// re-opened, lighter stream.
    async fn recover_by_shedding_images(&mut self) -> Result<bool, ApiError> {
        if self.committed {
            return Ok(false);
        }
        let Some(request) = shed_image_payload(&self.request) else {
            return Ok(false);
        };
        let Ok(transport) = self.client.open_transport(&request, false).await else {
            return Ok(false);
        };
        eprintln!(
            "[zo] gpt stream refused with no reason {} times; retrying once without the \
             {} image bytes it was carrying",
            self.restart_attempts.saturating_add(1),
            image_payload_bytes(&self.request),
        );
        self.request = request;
        self.transport = transport;
        self.state = ResponsesStreamState::new(self.request.model.clone())
            .with_session_id(self.client.cache_scope().to_string());
        self.pending.clear();
        self.done = false;
        self.reset_startup_watchdog();
        Ok(true)
    }

    /// Re-open the stream after a pre-commit fault: back off (jittered), then
    /// replace the live response and parser/state so the loop resumes from a
    /// clean turn. Any partial bytes buffered in the old parser are discarded
    /// with it — safe because nothing has been surfaced.
    async fn restart(&mut self, last_error: ApiError) -> Result<(), ApiError> {
        // Stamp the start of the restart sequence on the first restart so the
        // wall-clock budget in `can_restart` measures the whole storm.
        self.restart_window_start.get_or_insert_with(std::time::Instant::now);
        self.restart_attempts += 1;
        let base = self.client.backoff_for_attempt(self.restart_attempts)?;
        let delay = super::retry_backoff::spread_backoff(base);
        eprintln!(
            "[zo] gpt stream stalled ({last_error}); restarting in {:.1}s (attempt {}/{})",
            delay.as_secs_f64(),
            self.restart_attempts,
            self.client.max_retries,
        );
        // Surface the otherwise-silent reconnect pause to a live UI so it reads
        // as "reconnecting", not a freeze. The classifier label is shared with
        // the establish-time retry notice so the wording stays in lockstep.
        if let Some(notice) = &self.retry_notice {
            notice(StreamRetryNotice {
                kind: core_types::StreamNoticeKind::Reconnect,
                label: core_types::retry_signal::retry_notice_label(&last_error.to_string()),
                attempt: self.restart_attempts,
                max_attempts: self.client.max_retries,
                delay,
            });
        }
        tokio::time::sleep(delay).await;
        self.transport = self.client.open_transport(&self.request, false).await?;
        self.state = ResponsesStreamState::new(self.request.model.clone())
            .with_session_id(self.client.cache_scope().to_string());
        self.pending.clear();
        self.done = false;
        self.reset_startup_watchdog();
        Ok(())
    }
}

/// Stand-in text for a terminal failure frame that carried no message. The
/// `terminal stream failure` substring is load-bearing: [`is_terminal_stream_failure`]
/// keys the pre-commit recovery on it, and `core_types::retry_signal` classifies
/// it as transient. Anything appended must keep it contiguous.
const TERMINAL_STREAM_FAILURE: &str = "backend reported a terminal stream failure";

/// Diagnostic fields lifted into [`failure_frame_digest`], as display label and
/// the `serde_json` pointer each is read from. Strictly allowlisted: a terminal
/// failure frame is provider-controlled, so a denylist can never prove that
/// prompts, model output, tool payloads, or nested error details stay out of an
/// error string that gets displayed, logged, and persisted to the transcript.
const FAILURE_FRAME_DIGEST_FIELDS: [(&str, &str); 8] = [
    ("type", "/type"),
    ("code", "/code"),
    ("param", "/param"),
    ("status", "/response/status"),
    ("error.code", "/response/error/code"),
    ("error.type", "/response/error/type"),
    ("error.param", "/response/error/param"),
    ("incomplete", "/response/incomplete_details/reason"),
];

/// Longest rendered value kept per [`FAILURE_FRAME_DIGEST_FIELDS`] entry. With a
/// fixed field count this bounds the whole digest.
const FAILURE_FRAME_FIELD_MAX_LEN: usize = 80;

/// Digest of a terminal failure frame, used only when the backend named neither
/// an error code nor a message — without it the turn dies with a bare
/// [`TERMINAL_STREAM_FAILURE`] and nothing to tell a refused request from a
/// transient fault.
fn failure_frame_digest(event: &Value) -> Option<String> {
    let parts: Vec<String> = FAILURE_FRAME_DIGEST_FIELDS
        .iter()
        .filter_map(|(label, pointer)| {
            let value = event.pointer(pointer)?;
            // Scalars only. An object or array under an allowlisted pointer is
            // still provider-controlled content, and rendering it whole would
            // reopen the leak the allowlist exists to close.
            if !matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
                return None;
            }
            // `to_string` quotes and escapes, so a value carrying newlines or
            // control bytes cannot break out of the digest.
            Some(format!("{label}={}", truncate_digest_value(&value.to_string())))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn truncate_digest_value(rendered: &str) -> String {
    if rendered.len() <= FAILURE_FRAME_FIELD_MAX_LEN {
        return rendered.to_string();
    }
    let cut = (0..=FAILURE_FRAME_FIELD_MAX_LEN)
        .rev()
        .find(|index| rendered.is_char_boundary(*index))
        .unwrap_or(0);
    format!("{}…", &rendered[..cut])
}

/// Smallest image payload worth shedding, in base64 bytes.
///
/// Anchored to what a real screenshot actually weighs rather than to a round
/// number: the wire-lowering guard clamps every image to 2000px per side
/// (`runtime::image_guard::IMAGE_CLAMP_DIMENSION`), and a clamped full-page PNG
/// capture lands in the high hundreds of kilobytes once base64 inflates it by
/// 4/3. One megabyte therefore means "more than a single screenshot's worth of
/// pixels is riding along" — the accumulation case this exists for — while a
/// pasted avatar, an icon, or one small capture stays far below it and can never
/// trigger a shed.
const IMAGE_SHED_MIN_BYTES: usize = 1024 * 1024;

/// Base64 bytes the request spends on images, counting both directly attached
/// images and the ones staged inside tool results (where browser/MCP screenshot
/// tools put them, and from where every later request replays them).
fn image_payload_bytes(request: &MessageRequest) -> usize {
    payload_split(request).0
}

/// `(image bytes, everything else)` for the request, both measured in the units
/// that actually reach the wire: base64 for images, UTF-8 for text.
fn payload_split(request: &MessageRequest) -> (usize, usize) {
    use crate::types::ToolResultContentBlock;

    let mut images = 0usize;
    let mut other = request
        .system
        .iter()
        .flatten()
        .map(|block| match block {
            crate::types::SystemBlock::Text { text, .. } => text.len(),
        })
        .sum::<usize>();
    for message in &request.messages {
        for block in &message.content {
            match block {
                InputContentBlock::Image { source, .. } => images += source.data.len(),
                InputContentBlock::Text { text, .. } => other += text.len(),
                InputContentBlock::ToolUse { input, .. } => other += input.to_string().len(),
                InputContentBlock::ToolResult { content, .. } => {
                    for block in content {
                        match block {
                            ToolResultContentBlock::Image { source } => images += source.data.len(),
                            ToolResultContentBlock::Text { text } => other += text.len(),
                            ToolResultContentBlock::Json { value } => {
                                other += value.to_string().len();
                            }
                        }
                    }
                }
                InputContentBlock::Document { .. }
                | InputContentBlock::Thinking { .. }
                | InputContentBlock::RedactedThinking { .. } => {}
            }
        }
    }
    (images, other)
}

/// The request with its image pixels replaced by the same `[image <type>]`
/// placeholder [`super::flatten_tool_result_content`] already renders, or `None`
/// when shedding cannot plausibly be what the backend was refusing.
///
/// Two conditions must hold together, because either alone is a guess: the
/// images must clear [`IMAGE_SHED_MIN_BYTES`] in absolute terms, AND they must
/// be at least a third of the request. A turn whose bulk is prose or tool output
/// is not one that shrinking images can rescue, so it keeps the ordinary failure
/// path instead.
///
/// A third, not a half: a browser/MCP session stores a big *text* artifact
/// beside each screenshot (accessibility snapshot, DOM dump, network listing),
/// so the very sessions this exists for routinely carry more text than pixels.
/// Demanding an outright majority would decline exactly the case that motivated
/// it. A third still means the images are a leading term, and the cost of being
/// wrong is bounded — one extra request, then the original error.
///
/// Blocks are REPLACED, never removed: item count and order, `call_id` pairing,
/// and the flattened `function_call_output` text all stay byte-identical, so the
/// Responses `input` array a shed request builds differs from the original in
/// exactly one way — the `input_image` items are gone. The model still reads
/// that an image was there.
fn shed_image_payload(request: &MessageRequest) -> Option<MessageRequest> {
    use crate::types::ToolResultContentBlock;

    let (images, other) = payload_split(request);
    // `images >= (images + other) / 3`, written so it cannot overflow.
    if images < IMAGE_SHED_MIN_BYTES || images.saturating_mul(2) < other {
        return None;
    }
    let mut next = request.clone();
    for message in &mut next.messages {
        for block in &mut message.content {
            match block {
                InputContentBlock::Image { source, .. } => {
                    *block = InputContentBlock::Text {
                        text: image_placeholder(&source.media_type),
                        cache_control: None,
                    };
                }
                InputContentBlock::ToolResult { content, .. } => {
                    for block in content.iter_mut() {
                        if let ToolResultContentBlock::Image { source } = block {
                            *block = ToolResultContentBlock::Text {
                                text: image_placeholder(&source.media_type),
                            };
                        }
                    }
                }
                InputContentBlock::Text { .. }
                | InputContentBlock::Document { .. }
                | InputContentBlock::ToolUse { .. }
                | InputContentBlock::Thinking { .. }
                | InputContentBlock::RedactedThinking { .. } => {}
            }
        }
    }
    Some(next)
}

/// The placeholder an image degrades to. Byte-identical to what
/// [`super::flatten_tool_result_content`] writes for an image block, so a shed
/// tool result flattens to the same `function_call_output` string as before.
fn image_placeholder(media_type: &str) -> String {
    format!("[image {media_type}]")
}

fn is_terminal_stream_failure(error: &ApiError) -> bool {
    let ApiError::StreamApi { error_type, message, body, .. } = error else {
        return false;
    };
    let parts = [error_type.as_deref(), message.as_deref(), Some(body.as_str())];
    parts.iter().flatten().any(|part| {
        let lower = part.to_ascii_lowercase();
        lower.contains("terminal stream failure")
            || lower.contains("response.failed")
            || lower.contains("stream failed")
    })
}

fn deescalated_recovery_request(request: &MessageRequest) -> MessageRequest {
    let mut next = request.clone();
    next.stream = false;
    next.effort = request.effort.map(|effort| match effort {
        EffortLevel::Xhigh | EffortLevel::Max | EffortLevel::Ultra => EffortLevel::High,
        EffortLevel::Low | EffortLevel::Medium | EffortLevel::High => effort,
    });
    // A banded request's floor (Xhigh) just got forced down to a static High
    // above — clear the ceiling too, or the leftover band would let the wire
    // seam's `resolve_effort_band` re-escalate this deliberately-deescalated
    // recovery attempt right back past High on a heavy-intent/large-context
    // signal, undoing the point of this function.
    next.effort_band_ceiling = None;
    next.thinking = None;
    next.output_config = None;
    next
}

fn complete_response_events(response: MessageResponse) -> VecDeque<StreamEvent> {
    let usage = response.usage;
    let stop_reason = response.stop_reason.clone();
    let stop_sequence = response.stop_sequence.clone();
    VecDeque::from(vec![
        StreamEvent::MessageStart(MessageStartEvent { message: response }),
        StreamEvent::MessageDelta(MessageDeltaEvent {
            delta: MessageDelta {
                stop_reason,
                stop_sequence,
                thought_signature: None,
                reasoning_replay: None,
            },
            usage,
            context_management: None,
        }),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ])
}

/// Assemble the aggregate reasoning-replay JSON value from a Responses
/// `output` array and record each per-call entry in the session-scoped replay
/// cache fallback (see [`ReasoningReplayStore`]). Shared by the streaming
/// completed-response path ([`ResponsesStreamState::completed_output_deltas`])
/// and the true non-streaming [`parse_responses_response`].
///
/// The value is an array of two entry kinds:
/// * `{"call_id": .., "items": [..]}` — one per `function_call`, carrying the
///   `reasoning` items that immediately preceded it in output order.
/// * `{"turn_final": true, "items": [..]}` — appended last, carrying the
///   `reasoning` items that preceded the turn's final assistant `message`
///   (i.e. the reasoning that trails the last `function_call`, or the whole
///   turn's reasoning when the response made no call at all).
///
/// The turn-final entry is *additive*: it is appended after the per-call
/// entries and identified by a key old builds never look at
/// ([`reasoning_replay_from_attached`] matches on `call_id` only, and an entry
/// with no `call_id` can never match), so a session written by this build
/// replays per-call exactly as before under an older binary. Conversely a
/// session written by an older build simply has no turn-final entry and the
/// turn-final lookup yields `None`.
///
/// Capture is deliberately conservative in two places, because a `reasoning`
/// item replayed in `input` with the item it originally preceded *absent* is a
/// 400 (`Item 'rs_…' of type 'reasoning' was provided without its required
/// following item`):
/// * trailing reasoning is only captured once a `message` item is actually
///   seen after it — a response that ends in reasoning (e.g. an `incomplete`
///   response that spent its whole budget thinking) captures nothing;
/// * reasoning that precedes a `message` which is itself followed by a
///   `function_call` is dropped rather than misplaced, since replay has exactly
///   one turn-final position (before the message flushed at the end of the
///   assistant branch) and that mid-turn message is flushed earlier.
fn reasoning_replay_from_output(output: &[Value], session_id: &str) -> Option<Value> {
    let mut pending = Vec::new();
    let mut entries = Vec::new();
    let mut turn_final: Vec<Value> = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "reasoning" => pending.push(item.clone()),
            "function_call" => {
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let items = std::mem::take(&mut pending);
                if !call_id.is_empty() && !items.is_empty() {
                    entries.push(json!({
                        "call_id": call_id,
                        "items": items.clone(),
                    }));
                }
                cache_reasoning_for_call(session_id, call_id, items);
                // Anything attributed to an earlier `message` is no longer
                // turn-final: that message is flushed before this call, so its
                // reasoning has no replay position. Drop it (status quo)
                // instead of emitting it in the wrong place.
                turn_final.clear();
            }
            "message" => turn_final.append(&mut pending),
            _ => {}
        }
    }
    if !turn_final.is_empty() {
        entries.push(json!({
            TURN_FINAL_REPLAY_KEY: true,
            "items": turn_final,
        }));
    }
    (!entries.is_empty()).then_some(Value::Array(entries))
}

/// Parse a non-streaming Responses payload into a zo [`MessageResponse`].
fn parse_responses_response(value: &Value, model: &str, session_id: &str) -> MessageResponse {
    let mut content = Vec::new();
    if let Some(output) = value.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                "message" => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                content.push(OutputContentBlock::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                    }
                }
                "function_call" => content.push(OutputContentBlock::ToolUse {
                    id: str_field(item, "call_id"),
                    name: str_field(item, "name"),
                    input: item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|raw| serde_json::from_str(raw).ok())
                        .unwrap_or_else(|| json!({})),
                }),
                _ => {}
            }
        }
    }
    let usage = value.get("usage");
    // Honest stop reason for an incomplete non-streaming response (the model
    // spent its output budget, typically on reasoning): mirrors the streaming
    // path's `response.incomplete` mapping.
    let stop_reason = if value.get("status").and_then(Value::as_str) == Some("incomplete")
        && value
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
            == Some("max_output_tokens")
    {
        "max_tokens"
    } else {
        "end_turn"
    };
    MessageResponse {
        id: str_field(value, "id"),
        kind: "message".to_string(),
        role: "assistant".to_string(),
        content,
        model: model.to_string(),
        stop_reason: Some(stop_reason.to_string()),
        stop_sequence: None,
        usage: {
            let cache_read_input_tokens = usage_cached_tokens(usage);
            Usage {
                input_tokens: usage_field(usage, "input_tokens")
                    .saturating_sub(cache_read_input_tokens),
                cache_creation_input_tokens: 0,
                cache_read_input_tokens,
                output_tokens: usage_field(usage, "output_tokens"),
                output_tokens_details: None,
            }
        },
        request_id: None,
        thought_signature: None,
        reasoning_replay: value
            .get("output")
            .and_then(Value::as_array)
            .and_then(|output| reasoning_replay_from_output(output, session_id)),
        context_management: None,
    }
}

/// Diagnostic dump for the undocumented ChatGPT backend. When
/// `ZO_CHATGPT_DEBUG` is set, writes the request body / error response to
/// `/tmp/zo-chatgpt-<tag>.txt` so the exact wire shape can be compared
/// against codex. No-op (and silent on failure) otherwise — never affects the
/// request itself.
fn debug_dump_chatgpt(tag: &str, content: &str) {
    super::debug_dump("ZO_CHATGPT_DEBUG", "chatgpt", tag, content);
}

/// ChatGPT's code for an exhausted plan window — the wall only the clock, or
/// another provider, lifts.
pub(super) const USAGE_LIMIT_CODE: &str = "usage_limit_reached";

/// A usage-limit refusal as what it is: this account's 429, carrying the reset
/// the refusal named (or the one zo measured), and never retried in-stream.
///
/// It used to be a retryable `StreamApi` frame — the websocket treated
/// `usage_limit_reached` as "open a fresh connection", the ladder spent seven
/// reconnects on a window hours from resetting, and the exhausted error
/// classified as transient, so the quota escape never ran and the turn died.
/// As a 429 the structured classifier reads the account scope from the status,
/// the retry layer sees the reset in the head, and the conversation layer holds
/// or falls back exactly as it does for an HTTP 429.
pub(super) fn usage_limit_error(
    message: Option<String>,
    body: String,
    reset: Option<std::time::Duration>,
) -> ApiError {
    ApiError::Api {
        status: reqwest::StatusCode::TOO_MANY_REQUESTS,
        error_type: Some(USAGE_LIMIT_CODE.to_string()),
        message,
        body,
        retryable: false,
        retry_after: reset,
    }
}

/// The reset a usage-limit frame names, else the one zo measured for this
/// account's window (`measured` is asked only when the frame is silent).
pub(super) fn usage_limit_reset(
    frame: &Value,
    measured: impl FnOnce() -> Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    crate::reset_hint::reset_hint_in_json(frame, crate::quota::now_unix_secs()).or_else(measured)
}

/// The measured reset of this account's exhausted ChatGPT window, if known.
pub(super) fn measured_usage_reset() -> Option<std::time::Duration> {
    crate::quota::measured_reset_wait(super::ProviderKind::OpenAi)
}

async fn expect_success(response: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let header_retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(std::time::Duration::from_secs);
    let body = response.text().await.unwrap_or_default();
    debug_dump_chatgpt("error", &format!("status {status}\n{body}"));
    // Parse the OpenAI `{"error":{...}}` envelope so the transcript shows the
    // human-readable message, not the raw JSON body (parity with the API-key
    // `openai_compat` path).
    let parsed = serde_json::from_str::<super::openai_compat::ErrorEnvelope>(&body).ok();
    let error_type = parsed
        .as_ref()
        .and_then(|e| e.error.error_type.clone().or_else(|| e.error.code.clone()));
    let message = parsed.as_ref().and_then(|e| e.error.message.clone());
    // The header first, then the reset the body names, then — for a usage
    // limit only — the window zo measured: a 429 that says when it lifts lets
    // the retry layer skip a ride-out that cannot reach the reset.
    let retry_after = header_retry_after
        .or_else(|| crate::reset_hint::reset_hint_in_body(&body, crate::quota::now_unix_secs()))
        .or_else(|| {
            (error_type.as_deref() == Some(USAGE_LIMIT_CODE))
                .then(measured_usage_reset)
                .flatten()
        });
    Err(ApiError::Api {
        status,
        error_type,
        message,
        body,
        // Match the SDK `shouldRetry`: 408/409/429 plus every server error
        // (>= 500), so 529 overload is retried transparently, not surfaced.
        retryable: matches!(status.as_u16(), 408 | 409 | 429) || status.as_u16() >= 500,
        retry_after,
    })
}

/// Best-effort random session id for the `session_id` header. Falls back to a
/// fixed value if `/dev/urandom` is unavailable.
fn random_session_id() -> String {
    use std::fmt::Write as _;
    use std::io::Read as _;
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok()
    {
        bytes.iter().fold(String::new(), |mut acc, byte| {
            let _ = write!(acc, "{byte:02x}");
            acc
        })
    } else {
        "zo-chatgpt-session".to_string()
    }
}

mod websocket;

#[cfg(test)]
mod tests;
