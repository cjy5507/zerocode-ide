//! The `/responses` road for an OpenAI API key (t-121998): the wire a request
//! is put on, the body, the two refusals read, and what one endpoint said.

use super::person_turn_tests::{
    EFFORT_REFUSAL, MODEL, TOOLS_REFUSAL, bash_tool, client_at, drain, effort_on, fake_endpoint,
    like_openai, sse, tool_calls_of, turn, usage, user, with_subscription_row,
};
use super::responses_wire::{
    self, Learned, ResponsesDecoder, Wire, api_responses_body, clamp_effort, effort_in,
    efforts_the_refusal_names, responses_endpoint, sends_tools_to_responses, wire_for,
};
use super::{OpenAiCompatConfig, build_chat_completion_request};
use crate::error::ApiError;
use crate::types::{EffortLevel, StreamEvent};
use serde_json::json;

/// Before anything was said: OpenAI's own API puts a reasoning model with
/// function tools on `/responses` from the first request, and every other
/// request stays on Chat.
#[test]
fn the_wire_a_request_goes_on() {
    let _env = with_subscription_row();
    let nothing = Learned::default();
    let official = "https://api.openai.com/v1";
    let with_tools = turn(vec![user("hi")]);
    let mut without_tools = turn(vec![user("hi")]);
    without_tools.tools = None;
    let mut empty_tools = turn(vec![user("hi")]);
    empty_tools.tools = Some(Vec::new());
    let mut not_reasoning = turn(vec![user("hi")]);
    not_reasoning.model = "gpt-4o".to_string();

    assert_eq!(wire_for(&with_tools, official, &nothing), Wire::Responses);
    assert_eq!(wire_for(&with_tools, "https://api.openai.com/v1/", &nothing), Wire::Responses);
    assert_eq!(
        wire_for(&with_tools, "https://api.openai.com/v1/chat/completions", &nothing),
        Wire::Responses
    );
    assert_eq!(wire_for(&without_tools, official, &nothing), Wire::Chat);
    assert_eq!(wire_for(&empty_tools, official, &nothing), Wire::Chat);
    assert_eq!(wire_for(&not_reasoning, official, &nothing), Wire::Chat);
    for gateway in ["https://gateway.example.com/v1", "https://api.deepseek.com", "http://localhost:11434/v1"] {
        assert_eq!(wire_for(&with_tools, gateway, &nothing), Wire::Chat, "{gateway}");
    }
    // Once an endpoint said so, its tools go to `/responses` there too.
    let said = Learned { efforts: None, responses: true };
    assert_eq!(wire_for(&with_tools, "https://gateway.example.com/v1", &said), Wire::Responses);
    assert_eq!(wire_for(&without_tools, "https://gateway.example.com/v1", &said), Wire::Chat);
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

/// The first request of the person's turn on OpenAI's own API: `/responses`
/// with the API's fields, and the effort the catalog row allows (a refusal
/// then lowers it once).
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

#[test]
fn an_effort_refusal_names_the_rungs_the_model_takes() {
    let refusal = |message: &str| ApiError::Api {
        status: reqwest::StatusCode::BAD_REQUEST,
        error_type: Some("invalid_request_error".to_string()),
        message: Some(message.to_string()),
        body: json!({"error": {"message": message}}).to_string(),
        retryable: false,
        retry_after: None,
    };
    let takes = vec![EffortLevel::Low, EffortLevel::Medium, EffortLevel::High, EffortLevel::Xhigh];
    assert_eq!(efforts_the_refusal_names(&refusal(EFFORT_REFUSAL)), Some(takes.clone()));
    // Responses names its field `reasoning.effort`; the words around it were
    // not seen yet, so only the field and the list are read.
    assert_eq!(
        efforts_the_refusal_names(&refusal(&EFFORT_REFUSAL.replace("'reasoning_effort'", "'reasoning.effort'"))),
        Some(takes.clone())
    );
    assert_eq!(
        efforts_the_refusal_names(&refusal("Invalid value: 'max' for 'reasoning.effort'. Supported values are: 'low', 'medium', 'high', and 'xhigh'.")),
        Some(takes)
    );
    // Another field's unsupported value, another refusal, another status.
    assert_eq!(
        efforts_the_refusal_names(&refusal("Unsupported value: 'temperature' does not support 0.2 with this model. Supported values are: '1'.")),
        None
    );
    assert_eq!(efforts_the_refusal_names(&refusal(TOOLS_REFUSAL)), None);
    let unauthorized = ApiError::Api {
        status: reqwest::StatusCode::UNAUTHORIZED,
        error_type: None,
        message: Some(EFFORT_REFUSAL.to_string()),
        body: String::new(),
        retryable: false,
        retry_after: None,
    };
    assert_eq!(efforts_the_refusal_names(&unauthorized), None);

    // The rung a body carries, on either wire.
    assert_eq!(effort_in(&json!({"reasoning_effort": "max"})), Some(EffortLevel::Max));
    assert_eq!(effort_in(&json!({"reasoning": {"effort": "xhigh"}})), Some(EffortLevel::Xhigh));
    assert_eq!(effort_in(&json!({"model": MODEL})), None);

    assert!(sends_tools_to_responses(&refusal(TOOLS_REFUSAL)));
    assert!(!sends_tools_to_responses(&refusal(EFFORT_REFUSAL)));
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

/// OpenAI's own API puts the person's first request on `/responses` at the
/// catalog row's `max`; a refusal of `reasoning.effort` sends it again once at
/// the highest rung it names. The official address is the one wire decision a
/// loopback fake cannot take, so the endpoint's having said that its tools go
/// to `/responses` stands in for it here.
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
