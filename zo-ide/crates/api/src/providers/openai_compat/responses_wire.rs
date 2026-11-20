//! OpenAI's public Responses API (`POST {base}/responses`) for an API key.
//!
//! OpenAI's `/chat/completions` refuses function tools together with
//! `reasoning_effort` on its newer reasoning models — on 2026-11-20, a person
//! with an OpenAI API key on `gpt-6.2-astra` got `Function tools with
//! reasoning_effort are not supported for gpt-6.2-astra in /v1/chat/completions.
//! To use function tools, use /v1/responses or set reasoning_effort to 'none'.`
//! on every step. Before that the same request was refused for its effort:
//! `Unsupported value: 'reasoning_effort' does not support 'max' with this
//! model. Supported values are: 'low', 'medium', 'high', and 'xhigh'.` — the
//! model's catalog row had come from the ChatGPT subscription's model list,
//! which offers rungs the API does not take.
//!
//! This module is that road: which request goes on which wire, the body for
//! `/responses` (the subscription backend's own builder, with what the public
//! API does differently taken out), its stream, and what an endpoint's two
//! refusals above teach this process about one model. Each refusal is answered
//! once per request, and only when it says what one of the two above says —
//! the effort field and the values it takes, or function tools sent to
//! `/v1/responses`; what it taught is kept for the rest of the process, never
//! written to disk.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde_json::{Value, json};

use super::OpenAiCompatConfig;
use crate::error::ApiError;
use crate::providers::chatgpt_backend::{
    ResponsesSseParser, ResponsesStreamState, build_responses_body,
};
use crate::types::{EffortLevel, MessageRequest, MessageResponse, StreamEvent, SystemBlock, Usage};

/// The wire one request goes out on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Wire {
    /// `POST {base}/chat/completions`.
    Chat,
    /// `POST {base}/responses`.
    Responses,
}

/// What one endpoint said about one model while this process ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Learned {
    /// The rungs the endpoint said the model takes, after it refused one.
    pub(super) efforts: Option<Vec<EffortLevel>>,
    /// The endpoint said the model's function tools go to `/responses`.
    pub(super) responses: bool,
}

fn learned_by_endpoint() -> &'static Mutex<HashMap<String, Learned>> {
    static LEARNED: OnceLock<Mutex<HashMap<String, Learned>>> = OnceLock::new();
    LEARNED.get_or_init(Mutex::default)
}

fn learned_key(base_url: &str, model: &str) -> String {
    format!("{}\n{}", endpoint_base(base_url), model.trim().to_ascii_lowercase())
}

/// What the endpoint at `base_url` has said about `model` so far.
pub(super) fn learned(base_url: &str, model: &str) -> Learned {
    learned_by_endpoint()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&learned_key(base_url, model))
        .cloned()
        .unwrap_or_default()
}

fn learn(base_url: &str, model: &str, change: impl FnOnce(&mut Learned)) {
    let mut learned = learned_by_endpoint()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    change(learned.entry(learned_key(base_url, model)).or_default());
}

/// Keep the rungs `accepted` as the ones `model` takes at `base_url`.
pub(super) fn learn_efforts(base_url: &str, model: &str, accepted: Vec<EffortLevel>) {
    learn(base_url, model, |learned| learned.efforts = Some(accepted));
}

/// Keep that `model`'s function tools go to `{base}/responses` at `base_url`.
pub(super) fn learn_responses(base_url: &str, model: &str) {
    learn(base_url, model, |learned| learned.responses = true);
}

/// The base a person wrote, whichever way: `https://api.openai.com/v1` for
/// `…/v1/` and `…/v1/chat/completions` alike.
fn endpoint_base(base_url: &str) -> &str {
    let trimmed = base_url.trim_end_matches('/');
    trimmed.strip_suffix("/chat/completions").unwrap_or(trimmed)
}

/// `{base}/responses`, beside the `{base}/chat/completions` the Chat wire posts to.
pub(super) fn responses_endpoint(base_url: &str) -> String {
    format!("{}/responses", endpoint_base(base_url))
}

/// The wire `request` goes out on.
///
/// `/responses` for a request with function tools when the endpoint already
/// said the model's tools go there, or, before it said anything, when the
/// endpoint is OpenAI's own API and the model is a reasoning model: the pair
/// OpenAI's `/chat/completions` refuses on its newer models, and the API
/// OpenAI documents for reasoning models with tools. Everything else stays on
/// Chat as it was — a gateway, `DeepSeek`, a local server, a model that does
/// not reason (`gpt-4o`), and any request without tools.
pub(super) fn wire_for(request: &MessageRequest, base_url: &str, learned: &Learned) -> Wire {
    let has_tools = request.tools.as_ref().is_some_and(|tools| !tools.is_empty());
    if has_tools
        && (learned.responses
            || (super::is_official_openai_base_url(base_url)
                && super::is_reasoning_model(&request.model)))
    {
        Wire::Responses
    } else {
        Wire::Chat
    }
}

/// The words of a 400, where they can be read: the error's own message, then
/// the body it came in.
fn refusal_texts(error: &ApiError) -> Vec<&str> {
    match error {
        ApiError::Api {
            status,
            message,
            body,
            ..
        } if *status == reqwest::StatusCode::BAD_REQUEST => {
            message.as_deref().into_iter().chain([body.as_str()]).collect()
        }
        _ => Vec::new(),
    }
}

/// The rungs a refusal of the effort says the model takes, read from OpenAI's
/// own words: `Unsupported value: 'reasoning_effort' does not support 'max'
/// with this model. Supported values are: 'low', 'medium', 'high', and
/// 'xhigh'.` The refusal has to name the effort field — `reasoning_effort` on
/// Chat, `reasoning.effort` on Responses, whose words were not seen yet, so
/// the sentence around the field is not required — and list the values after
/// `Supported values are:`. `None` for every other refusal.
pub(super) fn efforts_the_refusal_names(error: &ApiError) -> Option<Vec<EffortLevel>> {
    refusal_texts(error).into_iter().find_map(efforts_named_in)
}

fn efforts_named_in(text: &str) -> Option<Vec<EffortLevel>> {
    const LIST: &str = "Supported values are:";
    let names_the_field = ["'reasoning_effort'", "'reasoning.effort'"]
        .iter()
        .any(|field| text.contains(field));
    if !names_the_field {
        return None;
    }
    let list = &text[text.find(LIST)? + LIST.len()..];
    // The list ends at the sentence's full stop; no rung's name holds one.
    let list = list.split_once('.').map_or(list, |(named, _)| named);
    let mut accepted = Vec::new();
    for named in list.split('\'').skip(1).step_by(2) {
        if let Some(level) = crate::providers::parse_effort_level(named) {
            if !accepted.contains(&level) {
                accepted.push(level);
            }
        }
    }
    (!accepted.is_empty()).then_some(accepted)
}

/// The rung a built body carries, on either wire.
pub(super) fn effort_in(body: &Value) -> Option<EffortLevel> {
    ["/reasoning_effort", "/reasoning/effort"]
        .iter()
        .find_map(|pointer| body.pointer(pointer))
        .and_then(Value::as_str)
        .and_then(crate::providers::parse_effort_level)
}

/// Whether a refusal says the model's function tools go to `/v1/responses`,
/// in OpenAI's own words: `Function tools with reasoning_effort are not
/// supported for gpt-6.2-astra in /v1/chat/completions. To use function tools,
/// use /v1/responses or set reasoning_effort to 'none'.`
pub(super) fn sends_tools_to_responses(error: &ApiError) -> bool {
    refusal_texts(error).into_iter().any(|text| {
        text.contains("Function tools with reasoning_effort are not supported")
            && text.contains("use /v1/responses")
    })
}

/// The highest rung of `accepted` at or below `level`; `low` when none is.
fn highest_at_or_below(level: EffortLevel, accepted: &[EffortLevel]) -> EffortLevel {
    EffortLevel::LADDER[..=level.rung()]
        .iter()
        .rev()
        .copied()
        .find(|candidate| accepted.contains(candidate))
        .unwrap_or(EffortLevel::Low)
}

/// The wire spelling of a rung: `ultra` is a ChatGPT product mode, not a value
/// either API takes, so it goes out as the enum's top, `max` — the same
/// projection [`EffortLevel::gpt_for_model`] makes.
const fn effort_wire(level: EffortLevel) -> &'static str {
    match level {
        EffortLevel::Low => "low",
        EffortLevel::Medium => "medium",
        EffortLevel::High => "high",
        EffortLevel::Xhigh => "xhigh",
        EffortLevel::Max | EffortLevel::Ultra => "max",
    }
}

/// Lower the effort `body` carries — `reasoning_effort` on Chat,
/// `reasoning.effort` on Responses — to the highest rung `accepted` holds at
/// or below it. A rung `accepted` holds, and a body that carries no effort,
/// are left as they are.
pub(super) fn clamp_effort(body: &mut Value, accepted: &[EffortLevel]) {
    for pointer in ["/reasoning_effort", "/reasoning/effort"] {
        let Some(slot) = body.pointer_mut(pointer) else {
            continue;
        };
        let Some(sent) = slot.as_str().and_then(crate::providers::parse_effort_level) else {
            continue;
        };
        let kept = highest_at_or_below(sent, accepted);
        if kept != sent {
            *slot = json!(effort_wire(kept));
        }
    }
}

/// zo's system prompt as the Chat body carries it: the blocks joined, the
/// served model's identity put right.
fn instructions(request: &MessageRequest, config: OpenAiCompatConfig) -> String {
    let text = request.system.as_ref().map_or_else(String::new, |blocks| {
        blocks
            .iter()
            .map(|block| match block {
                SystemBlock::Text { text, .. } => text.as_str(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    if text.is_empty() {
        return text;
    }
    crate::providers::apply_non_anthropic_identity(&text, &request.model, config.identity_maker())
}

/// Whether this request's pictures go out as pictures: the endpoint's answer
/// as the Chat body reads it, and never for a model whose catalog row says it
/// reads none.
pub(super) fn supports_vision(request: &MessageRequest, config: OpenAiCompatConfig) -> bool {
    crate::providers::custom_models::vision_for_config(&config, &request.model)
        .unwrap_or(config.supports_vision)
        && crate::providers::model_supports_vision(&request.model)
}

/// The body of one `POST {base}/responses` on an API key.
///
/// The subscription backend's builder ([`build_responses_body`]: the `input`
/// items, function tools, the reasoning items sent back with
/// `store: false` + `include: ["reasoning.encrypted_content"]`, the effort),
/// with what differs on the public API:
///
/// * `model` is the id the Chat body carries (the catalog's provider-API road);
/// * `max_output_tokens` carries the request's output cap — the subscription
///   refuses the field, the API takes it (Chat sends it as
///   `max_completion_tokens`);
/// * no `service_tier` — the Chat body never asked this key for one;
/// * the prompt-cache fields are the ones this endpoint's Chat body carries;
/// * `instructions` is the Chat body's system text (no subscription-only
///   addendum), and absent when there is none;
/// * no `reasoning.summary`: OpenAI asks some organizations to verify before a
///   reasoning model sends summaries, and a request that asks for one is then
///   refused; the Chat body never asked for reasoning text either.
pub(super) fn api_responses_body(
    request: &MessageRequest,
    config: OpenAiCompatConfig,
    stream: bool,
) -> Value {
    let instructions = instructions(request, config);
    let mut body = build_responses_body(
        request,
        &instructions,
        stream,
        "",
        supports_vision(request, config),
    );
    if let Some(fields) = body.as_object_mut() {
        fields.insert(
            "model".to_string(),
            json!(crate::providers::wire_model_for_request(
                &request.model,
                request.reasoning_request()
            )),
        );
        fields.insert("max_output_tokens".to_string(), json!(request.max_tokens));
        fields.remove("service_tier");
        fields.remove("prompt_cache_key");
        fields.remove("prompt_cache_retention");
        if instructions.is_empty() {
            fields.remove("instructions");
        }
        if let Some(reasoning) = fields.get_mut("reasoning").and_then(Value::as_object_mut) {
            reasoning.remove("summary");
        }
    }
    super::apply_prompt_cache_controls(&mut body, request, config);
    body
}

/// Usage as this endpoint's Chat path counts it: where the endpoint is not
/// taken to report cache reads, they stay inside `input_tokens`, as on Chat.
fn count_as_chat(usage: &mut Usage, config: OpenAiCompatConfig) {
    if !config.supports_cache_tokens {
        usage.input_tokens = usage
            .input_tokens
            .saturating_add(usage.cache_read_input_tokens);
        usage.cache_read_input_tokens = 0;
    }
}

/// A non-streaming `/responses` answer as zo's response.
pub(super) fn api_response(
    value: &Value,
    model: &str,
    config: OpenAiCompatConfig,
) -> MessageResponse {
    note_the_answer(value);
    let mut response = crate::providers::chatgpt_backend::parse_responses_response(value, model, "");
    count_as_chat(&mut response.usage, config);
    response
}

/// Leave the answer's id, model, status and token counts where
/// [`super::DEBUG_ENV`] says, when it is set.
fn note_the_answer(response: &Value) {
    crate::providers::debug_dump_with(super::DEBUG_ENV, "openai-compat", "response", || {
        json!({
            "id": response["id"],
            "model": response["model"],
            "status": response["status"],
            "usage": response["usage"],
        })
        .to_string()
    });
}

/// One `/responses` stream read into zo's events, by the subscription
/// backend's own parser and state.
#[derive(Debug)]
pub(super) struct ResponsesDecoder {
    parser: ResponsesSseParser,
    state: ResponsesStreamState,
    config: OpenAiCompatConfig,
}

impl ResponsesDecoder {
    pub(super) fn new(model: String, config: OpenAiCompatConfig) -> Self {
        Self {
            parser: ResponsesSseParser::new(),
            state: ResponsesStreamState::new(model),
            config,
        }
    }

    /// The events one chunk completes, and the failure the stream reported in
    /// it, if it did. Events that came before the failure are kept: they are
    /// output the caller must still receive.
    pub(super) fn push(
        &mut self,
        chunk: &[u8],
    ) -> Result<(Vec<StreamEvent>, Option<ApiError>), ApiError> {
        let mut events = Vec::new();
        for value in self.parser.push(chunk)? {
            if matches!(
                value["type"].as_str(),
                Some("response.completed" | "response.incomplete" | "response.failed")
            ) {
                note_the_answer(&value["response"]);
            }
            events.extend(self.state.ingest(&value));
        }
        for event in &mut events {
            if let StreamEvent::MessageDelta(delta) = event {
                count_as_chat(&mut delta.usage, self.config);
            }
        }
        Ok((events, self.state.take_failure()))
    }
}
