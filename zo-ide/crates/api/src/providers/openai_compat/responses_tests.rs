//! The `/responses` road for an OpenAI API key (t-121998): the wire a request
//! is put on, the body, the two refusals read, and what one endpoint said.

use super::person_turn_tests::{
    EFFORT_REFUSAL, MODEL, TOOLS_REFUSAL, assistant_call, bash_tool, client_at, drain, effort_on,
    fake_endpoint, like_openai, sse, tool_calls_of, tool_result, turn, usage, user,
    with_subscription_row,
};
use crate::providers::chatgpt_backend::build_responses_request_for_session;
use super::responses_wire::{
    self, API_SCOPE, Learned, ResponsesDecoder, Wire, api_responses_body, clamp_effort,
    efforts_listed_in, efforts_to_send_again, lists_supported_values, responses_endpoint,
    sends_tools_to_responses, wire_for,
};
use super::{OpenAiCompatConfig, build_chat_completion_request};
use crate::error::ApiError;
use crate::types::{EffortLevel, InputContentBlock, InputMessage, StreamEvent};
use serde_json::{Value, json};

/// A request goes to `/responses` only once its endpoint refused its function
/// tools on Chat; OpenAI's own API included, every request goes on Chat
/// before that, as it did.
#[test]
fn the_wire_a_request_goes_on() {
    let _env = with_subscription_row();
    let nothing = Learned::default();
    let said = Learned { efforts: None, responses: true };
    let with_tools = turn(vec![user("hi")]);
    let mut without_tools = turn(vec![user("hi")]);
    without_tools.tools = None;
    let mut empty_tools = turn(vec![user("hi")]);
    empty_tools.tools = Some(Vec::new());
    let mut not_reasoning = turn(vec![user("hi")]);
    not_reasoning.model = "gpt-4o".to_string();

    for request in [&with_tools, &without_tools, &empty_tools, &not_reasoning] {
        assert_eq!(wire_for(request, &nothing), Wire::Chat, "{}", request.model);
    }
    assert_eq!(wire_for(&with_tools, &said), Wire::Responses);
    assert_eq!(wire_for(&not_reasoning, &said), Wire::Responses);
    assert_eq!(wire_for(&without_tools, &said), Wire::Chat);
    assert_eq!(wire_for(&empty_tools, &said), Wire::Chat);
}

#[test]
fn the_responses_address_sits_beside_chat() {
    for base in [
        "https://api.openai.com/v1",
        "https://api.openai.com/v1/",
        "https://api.openai.com/v1/chat/completions",
    ] {
        assert_eq!(responses_endpoint(base), "https://api.openai.com/v1/responses", "{base}");
    }
}

/// A `/responses` body on OpenAI's own config: the API's fields, and the
/// effort the catalog row allows (a refusal lowers it once).
#[test]
fn the_api_responses_body() {
    let _env = with_subscription_row();
    let request = turn(vec![user("hi")]);

    let body = api_responses_body(&request, OpenAiCompatConfig::openai(), true);
    assert_eq!(body["model"], json!(MODEL));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["store"], json!(false));
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["max_output_tokens"], json!(4096));
    assert_eq!(body["reasoning"], json!({"effort": "max"}));
    assert_eq!(body["tool_choice"], json!("auto"));
    assert_eq!(
        body["tools"],
        json!([{"type": "function", "name": "bash", "description": "Run a shell command",
            "parameters": bash_tool().input_schema, "strict": false}])
    );
    assert!(body.get("service_tier").is_none());
    // OpenAI's own config keys the prompt cache, as its Chat body does.
    assert_eq!(
        body.get("prompt_cache_key"),
        build_chat_completion_request(&request, OpenAiCompatConfig::openai()).get("prompt_cache_key")
    );
    let instructions = body["instructions"].as_str().expect("instructions");
    assert!(
        instructions.starts_with("You are gpt-6.2-astra, a large language model made by OpenAI"),
        "{instructions}"
    );
    assert!(instructions.ends_with("Follow the house rules."), "{instructions}");
    assert_eq!(body["input"], json!([{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}]));

    // A person's row asks for no prompt-cache control, on Chat or here.
    let row = OpenAiCompatConfig::from_user("OpenAI", "https://api.openai.com/v1", Some("ZO_TEST_OPENAI_KEY"), true);
    let body = api_responses_body(&request, row, true);
    assert!(body.get("prompt_cache_key").is_none(), "{body}");
    assert!(body.get("prompt_cache_retention").is_none(), "{body}");
    assert!(build_chat_completion_request(&request, row).get("prompt_cache_key").is_none());

    // No system text, no `instructions`.
    let mut bare = request.clone();
    bare.system = None;
    assert!(api_responses_body(&bare, row, false).get("instructions").is_none());
}

fn refusal(message: &str, param: &str) -> ApiError {
    ApiError::Api {
        status: reqwest::StatusCode::BAD_REQUEST,
        error_type: Some("invalid_request_error".to_string()),
        message: Some(message.to_string()),
        body: json!({"error": {"message": message, "type": "invalid_request_error", "param": param, "code": null}})
            .to_string(),
        retryable: false,
        retry_after: None,
    }
}

#[test]
fn an_effort_refusal_names_the_rungs_the_model_takes() {
    let takes = vec![EffortLevel::Low, EffortLevel::Medium, EffortLevel::High, EffortLevel::Xhigh];
    let chat_max = json!({"reasoning_effort": "max"});
    let responses_max = json!({"reasoning": {"effort": "max"}});

    // The person's Chat refusal, word for word.
    let person = refusal(EFFORT_REFUSAL, "reasoning_effort");
    assert!(lists_supported_values(&person));
    assert_eq!(efforts_to_send_again(&person, &chat_max), Some(takes.clone()));
    // The value that went out, quoted, with no field named — a gateway's
    // page's form — on either wire.
    let unnamed = refusal(
        "Unsupported value: 'max' is not supported with the 'gpt-6.2-astra' model. Supported values are: 'none', 'low', 'medium', 'high', and 'xhigh'.",
        "",
    );
    assert_eq!(efforts_to_send_again(&unnamed, &responses_max), Some(takes.clone()));
    assert_eq!(efforts_to_send_again(&unnamed, &chat_max), Some(takes.clone()));
    // The field only as the body's `param`.
    let by_param = refusal(
        "This model does not take that value. Supported values are: 'low', 'medium', 'high', and 'xhigh'.",
        "reasoning.effort",
    );
    assert_eq!(efforts_to_send_again(&by_param, &responses_max), Some(takes.clone()));
    // The gateway's page's words exactly, about the value `minimal`.
    assert_eq!(
        efforts_listed_in(
            "Unsupported value: 'minimal' is not supported with the 'gpt-5.6-terra' model. Supported values are: 'none', 'low', 'medium', 'high', 'xhigh', and 'max'.",
            "minimal",
            false,
        ),
        Some(vec![EffortLevel::Low, EffortLevel::Medium, EffortLevel::High, EffortLevel::Xhigh, EffortLevel::Max])
    );

    // Not sent again: a rung the refusal lists went out; another value was
    // refused; another field's list; a list that holds a value no effort is;
    // no effort in the body; another status.
    assert_eq!(efforts_to_send_again(&person, &json!({"reasoning_effort": "xhigh"})), None);
    assert_eq!(efforts_to_send_again(&unnamed, &json!({"reasoning": {"effort": "high"}})), None);
    let temperature = refusal(
        "Unsupported value: 'temperature' does not support 0.2 with this model. Supported values are: '1'.",
        "temperature",
    );
    assert!(lists_supported_values(&temperature));
    assert_eq!(efforts_to_send_again(&temperature, &chat_max), None);
    let mixed = refusal(
        "Unsupported value: 'max' is not supported with the 'gpt-6.2-astra' model. Supported values are: 'low', 'auto', and 'xhigh'.",
        "",
    );
    assert_eq!(efforts_to_send_again(&mixed, &chat_max), None);
    assert_eq!(efforts_to_send_again(&person, &json!({"model": MODEL})), None);
    let unauthorized = ApiError::Api {
        status: reqwest::StatusCode::UNAUTHORIZED,
        error_type: None,
        message: Some(EFFORT_REFUSAL.to_string()),
        body: String::new(),
        retryable: false,
        retry_after: None,
    };
    assert!(!lists_supported_values(&unauthorized));
    assert_eq!(efforts_to_send_again(&unauthorized, &chat_max), None);
    assert!(!lists_supported_values(&refusal(TOOLS_REFUSAL, "reasoning_effort")));

    assert!(sends_tools_to_responses(&refusal(TOOLS_REFUSAL, "reasoning_effort")));
    assert!(!sends_tools_to_responses(&person));
    assert!(!sends_tools_to_responses(&unauthorized));
}

#[test]
fn a_named_ceiling_lowers_the_effort_on_either_wire() {
    let takes = [EffortLevel::Low, EffortLevel::Medium, EffortLevel::High, EffortLevel::Xhigh];
    let mut chat = json!({"reasoning_effort": "max"});
    clamp_effort(&mut chat, &takes);
    assert_eq!(chat, json!({"reasoning_effort": "xhigh"}));
    let mut responses = json!({"reasoning": {"effort": "max"}});
    clamp_effort(&mut responses, &takes);
    assert_eq!(responses, json!({"reasoning": {"effort": "xhigh"}}));
    for kept in [json!({"reasoning_effort": "high"}), json!({"reasoning": {"effort": "low"}}), json!({"model": MODEL})] {
        let mut body = kept.clone();
        clamp_effort(&mut body, &takes);
        assert_eq!(body, kept);
    }
}

/// Cache reads are counted the way this endpoint's Chat path counts them.
#[test]
fn the_stream_counts_cache_reads_as_chat_does() {
    let completed = sse(&[json!({"type": "response.completed", "response": {"id": "resp_4", "status": "completed", "output": [], "usage": usage()}})]);
    let usage_after = |config| {
        let mut decoder = ResponsesDecoder::new(MODEL.to_string(), config);
        let (events, failure) = decoder.push(completed.as_bytes()).expect("read");
        assert!(failure.is_none());
        events
            .into_iter()
            .find_map(|event| match event {
                StreamEvent::MessageDelta(delta) => Some(delta.usage),
                _ => None,
            })
            .expect("the closing usage")
    };
    let openai = usage_after(OpenAiCompatConfig::openai());
    assert_eq!((openai.input_tokens, openai.cache_read_input_tokens), (20, 100));
    let row = usage_after(OpenAiCompatConfig::from_user("OpenAI", "https://api.openai.com/v1", None, true));
    assert_eq!((row.input_tokens, row.cache_read_input_tokens), (120, 0));
}

/// What one endpoint said is kept for that endpoint and model only.
#[test]
fn what_an_endpoint_said_is_kept_for_it_alone() {
    let base = "https://kept.example.com/v1";
    responses_wire::learn_responses(base, "gpt-kept-1");
    responses_wire::learn_efforts(base, "gpt-kept-1", vec![EffortLevel::Low]);
    assert_eq!(
        responses_wire::learned(&format!("{base}/chat/completions"), "GPT-KEPT-1"),
        Learned { efforts: Some(vec![EffortLevel::Low]), responses: true }
    );
    assert_eq!(responses_wire::learned(base, "gpt-kept-2"), Learned::default());
    assert_eq!(responses_wire::learned("https://other.example.com/v1", "gpt-kept-1"), Learned::default());
}

/// Two function calls in one answer stay two calls, each under its own id.
#[test]
fn parallel_calls_on_responses_stay_apart() {
    let call = |index: u64, id: &str| {
        let item = json!({"id": format!("fc_{index}"), "type": "function_call", "status": "completed", "call_id": id, "name": "bash", "arguments": "{\"command\":\"pwd\"}"});
        [
            json!({"type": "response.output_item.added", "output_index": index, "item": {"id": format!("fc_{index}"), "type": "function_call", "status": "in_progress", "call_id": id, "name": "bash", "arguments": ""}}),
            json!({"type": "response.function_call_arguments.delta", "output_index": index, "item_id": format!("fc_{index}"), "delta": "{\"command\":\"pwd\"}"}),
            json!({"type": "response.output_item.done", "output_index": index, "item": item}),
        ]
    };
    let mut events = vec![json!({"type": "response.created", "response": {"id": "resp_5", "status": "in_progress", "output": []}})];
    events.extend(call(0, "call_parallel_a"));
    events.extend(call(1, "call_parallel_b"));
    events.push(json!({"type": "response.completed", "response": {"id": "resp_5", "status": "completed", "output": [], "usage": usage()}}));
    let mut decoder = ResponsesDecoder::new(MODEL.to_string(), OpenAiCompatConfig::openai());
    let (out, failure) = decoder.push(sse(&events).as_bytes()).expect("read");
    assert!(failure.is_none());
    let ids: Vec<String> = out
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ContentBlockStart(start) => match &start.content_block {
                crate::types::OutputContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["call_parallel_a".to_string(), "call_parallel_b".to_string()]);
}

/// A pasted picture goes out as a picture where the endpoint takes pictures,
/// and as the Chat body's placeholder text where it does not.
#[test]
fn pictures_on_responses_follow_the_endpoint_as_on_chat() {
    let _env = with_subscription_row();
    let mut request = turn(Vec::new());
    request.messages = vec![crate::types::InputMessage {
        role: "user".to_string(),
        content: vec![crate::types::InputContentBlock::Image {
            source: crate::types::ImageSource {
                kind: "base64".to_string(),
                media_type: "image/png".to_string(),
                data: "iVBORw0KGgo=".to_string(),
            },
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: None,
    }];
    let official = api_responses_body(&request, OpenAiCompatConfig::openai(), true);
    assert_eq!(official["input"][0]["content"][0]["type"], json!("input_image"));
    let mut blind = OpenAiCompatConfig::openai();
    blind.supports_vision = false;
    let placeholder = api_responses_body(&request, blind, true);
    assert_eq!(placeholder["input"][0]["content"][0]["type"], json!("input_text"));
}

/// An effort refused on `/responses` itself — a request that reaches it at the
/// catalog row's `max` once the endpoint said its tools go there but before
/// any effort was refused — goes again once at the highest rung the refusal
/// lists. The fake refuses in the form a gateway's page quotes, naming no
/// field, since OpenAI's own words there were not seen.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_refused_effort_on_responses_goes_again_at_the_rung_it_names() {
    let _env = with_subscription_row();
    let (base_url, seen) = fake_endpoint(like_openai("call_t121998_b")).await;
    responses_wire::learn_responses(&base_url, MODEL);
    let events = drain(
        client_at(&base_url)
            .stream_message(&turn(vec![user("hi")]))
            .await
            .expect("the step is answered"),
    )
    .await
    .expect("the step streams to its end");
    assert_eq!(tool_calls_of(&events), vec![("call_t121998_b".to_string(), "bash".to_string())]);
    let seen = seen.lock().expect("seen");
    let route: Vec<(&str, Option<&str>)> =
        seen.iter().map(|request| (request.path.as_str(), effort_on(request))).collect();
    assert_eq!(route, vec![("/v1/responses", Some("max")), ("/v1/responses", Some("xhigh"))]);
}

/// A session that used the ChatGPT login before the key: the reasoning the
/// subscription left on its messages — before a call and before the turn's
/// last words — does not go to the API, while the subscription's own request
/// still carries it.
#[test]
fn the_subscription_s_reasoning_on_a_message_stays_off_the_api_key_s_request() {
    let _env = with_subscription_row();
    let call_id = "call_from_the_subscription";
    let attached = json!([
        {"call_id": call_id, "items": [{"type": "reasoning", "id": "rs_sub", "summary": [], "encrypted_content": "SUBSCRIPTION-ITEM"}]},
        {"turn_final": true, "items": [{"type": "reasoning", "id": "rs_sub_final", "summary": [], "encrypted_content": "SUBSCRIPTION-FINAL"}]},
    ]);
    let answered = InputMessage {
        role: "assistant".to_string(),
        content: vec![
            InputContentBlock::ToolUse {
                id: call_id.to_string(),
                name: "bash".to_string(),
                input: json!({"command": "pwd"}),
                cache_control: None,
            },
            InputContentBlock::Text { text: "Here it is.".to_string(), cache_control: None },
        ],
        thought_signature: None,
        reasoning_replay: Some(attached),
    };
    let request = turn(vec![user("hi"), answered, tool_result(call_id, "/w"), user("and now?")]);

    let subscription = build_responses_request_for_session(&request, "instructions", true, "session-sub").to_string();
    assert!(subscription.contains("SUBSCRIPTION-ITEM") && subscription.contains("SUBSCRIPTION-FINAL"));

    let api = api_responses_body(&request, OpenAiCompatConfig::openai(), true);
    let kinds: Vec<&str> = api["input"]
        .as_array()
        .expect("input items")
        .iter()
        .map(|item| item["type"].as_str().unwrap_or("message"))
        .collect();
    assert!(!kinds.contains(&"reasoning"), "{kinds:?}");
    assert!(kinds.contains(&"function_call") && kinds.contains(&"function_call_output"), "{kinds:?}");
    assert!(!api.to_string().contains("SUBSCRIPTION"), "{api}");
}

/// A session that used the key, then the ChatGPT login: nothing the key's road
/// made reaches the subscription's request, which is byte for byte what it was
/// before the key answered; the key's own next request carries it from this
/// process.
#[test]
fn the_api_key_s_reasoning_never_reaches_a_subscription_request() {
    let _env = with_subscription_row();
    let call_id = "call_from_the_api_key";
    let request = turn(vec![user("hi"), assistant_call(call_id, "pwd", None), tool_result(call_id, "/w")]);
    let before = build_responses_request_for_session(&request, "instructions", true, "session-sub").to_string();

    let reasoning = json!({"id": "rs_api", "type": "reasoning", "summary": [], "encrypted_content": "API-KEY-ITEM"});
    let call = json!({"id": "fc_api", "type": "function_call", "status": "completed", "call_id": call_id, "name": "bash", "arguments": "{\"command\":\"pwd\"}"});
    let mut decoder = ResponsesDecoder::new(MODEL.to_string(), OpenAiCompatConfig::openai());
    let (events, failure) = decoder
        .push(
            sse(&[
                json!({"type": "response.created", "response": {"id": "resp_api", "status": "in_progress", "output": []}}),
                json!({"type": "response.output_item.done", "output_index": 0, "item": reasoning}),
                json!({"type": "response.output_item.added", "output_index": 1, "item": call}),
                json!({"type": "response.output_item.done", "output_index": 1, "item": call}),
                json!({"type": "response.completed", "response": {"id": "resp_api", "status": "completed", "output": [reasoning, call], "usage": usage()}}),
            ])
            .as_bytes(),
        )
        .expect("read");
    assert!(failure.is_none());
    let replays: Vec<&Option<Value>> = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::MessageDelta(delta) => Some(&delta.delta.reasoning_replay),
            _ => None,
        })
        .collect();
    assert_eq!(replays, vec![&None], "nothing for the session's messages");

    let after = build_responses_request_for_session(&request, "instructions", true, "session-sub").to_string();
    assert_eq!(before, after);
    assert!(!after.contains("API-KEY-ITEM"));
    assert!(api_responses_body(&request, OpenAiCompatConfig::openai(), true).to_string().contains("API-KEY-ITEM"));
    assert_eq!(API_SCOPE, "openai-api-key-responses");
}

/// A session resumed in a new process: the key's road holds no reasoning for
/// its earlier calls, so a call goes back the way OpenAI's function-calling
/// guide sends one — `call_id`, `name`, `arguments`, no output `id` — with its
/// output, and no reasoning item.
#[test]
fn a_call_this_process_holds_no_reasoning_for_goes_back_without_it() {
    let _env = with_subscription_row();
    let call_id = "call_from_an_earlier_process";
    let request = turn(vec![user("hi"), assistant_call(call_id, "pwd", None), tool_result(call_id, "/w")]);
    let body = api_responses_body(&request, OpenAiCompatConfig::openai(), true);
    assert_eq!(
        body["input"],
        json!([
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
            {"type": "function_call", "call_id": call_id, "name": "bash", "arguments": "{\"command\":\"pwd\"}"},
            {"type": "function_call_output", "call_id": call_id, "output": "/w"},
        ])
    );
}
