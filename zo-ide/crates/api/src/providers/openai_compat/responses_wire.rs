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
//! This module is that road: what an endpoint's two refusals above teach this
//! process about one model, the body for `/responses` (the subscription
//! backend's own builder, with what the public API does differently taken
//! out), and its stream. A request goes to `/responses` only after its
//! endpoint refused function tools on Chat in OpenAI's words, so every request
//! that worked on Chat before goes there as it did. Each refusal is answered
//! once per request; what it taught is kept for the rest of the process, never
//! written to disk.
//!
//! The reasoning a model sends on this road rides on the session's messages
//! marked with the road that made it ([`api_road`]: this road and its
//! endpoint), and goes back only on that road: a ChatGPT login and an API key
//! are different OpenAI accounts, and so may two endpoints be, and the API
//! removes or refuses reasoning made under another organization (OpenAI's
//! reasoning guide, "Reasoning item removal across requests"). The
//! subscription's reasoning, which carries no mark, is not sent here, and
//! what this road marks is not sent to the subscription
//! (`chatgpt_backend::build_responses_body`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde_json::{Value, json};

use super::OpenAiCompatConfig;
use crate::error::ApiError;
use crate::providers::chatgpt_backend::{
    REPLAY_ROAD_KEY, ResponsesSseParser, ResponsesStreamState, build_responses_body,
};
use crate::types::{EffortLevel, MessageRequest, MessageResponse, StreamEvent, SystemBlock, Usage};

/// The road an API key's `/responses` answers come back on: this road and the
/// endpoint they came from. It marks the reasoning this road puts on the
/// session's messages, and names what this process keeps of it (the
/// subscription keeps its own under each client's session).
pub(super) fn api_road(base_url: &str) -> String {
    format!("openai-api-key {}", endpoint_base(base_url))
}

/// Mark every reasoning-replay entry of an answer with `road`.
fn mark_road(replay: &mut Option<Value>, road: &str) {
    if let Some(Value::Array(entries)) = replay.as_mut() {
        for entry in entries {
            if let Some(fields) = entry.as_object_mut() {
                fields.insert(REPLAY_ROAD_KEY.to_string(), json!(road));
            }
        }
    }
}

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

/// The wire `request` goes out on: `/responses` for a request with function
/// tools once its endpoint said, refusing them on Chat, that the model's tools
/// go there ([`sends_tools_to_responses`]); Chat for everything else, as
/// before — OpenAI's own API included, where the reasoning models that take
/// tools on Chat go on taking them there.
pub(super) fn wire_for(request: &MessageRequest, learned: &Learned) -> Wire {
    let has_tools = request.tools.as_ref().is_some_and(|tools| !tools.is_empty());
    if has_tools && learned.responses {
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

const SUPPORTED_VALUES: &str = "Supported values are:";
const EFFORT_FIELDS: [&str; 2] = ["reasoning_effort", "reasoning.effort"];

/// Whether a refusal lists the values a field takes — the shape an effort
/// refusal comes in, checked before the request it answers is built again.
pub(super) fn lists_supported_values(error: &ApiError) -> bool {
    refusal_texts(error)
        .into_iter()
        .any(|text| text.contains(SUPPORTED_VALUES))
}

/// The rungs to send the request again within, when `error` refuses the
/// effort the body `sent` carried; `None` for any other refusal, and when the
/// rung that went out is one the refusal lists (the same request twice would
/// be refused twice).
///
/// OpenAI's words on Chat (2026-11-20): `Unsupported value: 'reasoning_effort'
/// does not support 'max' with this model. Supported values are: 'low',
/// 'medium', 'high', and 'xhigh'.` Its words on `/v1/responses` were not seen,
/// and a gateway's page quotes the same refusal without the field:
/// `Unsupported value: 'minimal' is not supported with the 'gpt-5.6-terra'
/// model. Supported values are: 'none', 'low', 'medium', 'high', 'xhigh', and
/// 'max'.` So a refusal is the effort's when it names the effort field — in
/// its words or as its `param` — or quotes the value that went out, before a
/// list whose values are all efforts.
pub(super) fn efforts_to_send_again(error: &ApiError, sent: &Value) -> Option<Vec<EffortLevel>> {
    let sent_text = effort_text_in(sent)?;
    let sent_level = crate::providers::parse_effort_level(sent_text)?;
    let param_is_the_effort =
        refusal_param(error).is_some_and(|param| EFFORT_FIELDS.contains(&param.as_str()));
    let accepted = refusal_texts(error)
        .into_iter()
        .find_map(|text| efforts_listed_in(text, sent_text, param_is_the_effort))?;
    (!accepted.contains(&sent_level)).then_some(accepted)
}

/// The `param` of a refusal's `{"error": {...}}` body.
fn refusal_param(error: &ApiError) -> Option<String> {
    let ApiError::Api { body, .. } = error else {
        return None;
    };
    serde_json::from_str::<Value>(body)
        .ok()?
        .pointer("/error/param")?
        .as_str()
        .map(str::to_string)
}

/// The efforts `text` lists after `Supported values are:` when it is a
/// refusal of the effort `sent`.
pub(super) fn efforts_listed_in(
    text: &str,
    sent: &str,
    param_is_the_effort: bool,
) -> Option<Vec<EffortLevel>> {
    let at = text.find(SUPPORTED_VALUES)?;
    let (before, after) = (&text[..at], &text[at + SUPPORTED_VALUES.len()..]);
    let names_the_effort = param_is_the_effort
        || EFFORT_FIELDS
            .iter()
            .any(|field| text.contains(&format!("'{field}'")))
        || before.contains(&format!("'{sent}'"));
    if !names_the_effort {
        return None;
    }
    // The list ends at the sentence's full stop; no rung's name holds one.
    let list = after.split_once('.').map_or(after, |(named, _)| named);
    let mut accepted = Vec::new();
    for named in list.split('\'').skip(1).step_by(2) {
        // A value that is not an effort makes it another field's list.
        let level = crate::providers::parse_effort_level(named)?;
        if !accepted.contains(&level) {
            accepted.push(level);
        }
    }
    (!accepted.is_empty()).then_some(accepted)
}

/// The effort a built body carries, as it went out — `reasoning_effort` on
/// Chat, `reasoning.effort` on Responses.
fn effort_text_in(body: &Value) -> Option<&str> {
    ["/reasoning_effort", "/reasoning/effort"]
        .iter()
        .find_map(|pointer| body.pointer(pointer))
        .and_then(Value::as_str)
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
///   refused; the Chat body never asked for reasoning text either;
/// * the reasoning sent back is only what this road, at this endpoint, marked
///   ([`api_road`]; the module's note says why).
pub(super) fn api_responses_body(
    request: &MessageRequest,
    config: OpenAiCompatConfig,
    base_url: &str,
    stream: bool,
) -> Value {
    let instructions = instructions(request, config);
    let road = api_road(base_url);
    let mut body = build_responses_body(
        request,
        &instructions,
        stream,
        &road,
        supports_vision(request, config),
        Some(&road),
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
    base_url: &str,
) -> MessageResponse {
    note_the_answer(value);
    let road = api_road(base_url);
    let mut response =
        crate::providers::chatgpt_backend::parse_responses_response(value, model, &road);
    mark_road(&mut response.reasoning_replay, &road);
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
    road: String,
}

impl ResponsesDecoder {
    pub(super) fn new(model: String, config: OpenAiCompatConfig, base_url: &str) -> Self {
        let road = api_road(base_url);
        Self {
            parser: ResponsesSseParser::new(),
            state: ResponsesStreamState::new(model).with_session_id(road.clone()),
            config,
            road,
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
                mark_road(&mut delta.delta.reasoning_replay, &self.road);
            }
        }
        Ok((events, self.state.take_failure()))
    }
}
