use super::{
    ResponsesSseParser, ResponsesStreamState, build_responses_request, completed_response_from_sse,
    build_responses_request_for_session, cache_reasoning_for_call, crosses_restart_commit_boundary,
    de_escalated_effort, deescalated_recovery_request, parse_responses_response, reasoning_effort,
    reasoning_for_call, reasoning_replay_from_output, remove_reasoning_for_call,
};
use crate::providers::should_restart;
use crate::error::ApiError;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, EffortLevel, ContentBlockStopEvent, ImageSource,
    InputContentBlock, InputMessage, MessageRequest, OutputContentBlock, StreamEvent, SystemBlock,
    ThinkingConfig, ToolChoice, ToolDefinition,
};
use serde_json::json;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_env_lock()
}

fn request(
    messages: Vec<InputMessage>,
    tools: Option<Vec<ToolDefinition>>,
    thinking: Option<ThinkingConfig>,
) -> MessageRequest {
    MessageRequest {
        model: "gpt-5.6-sol".into(),
        max_tokens: 1000,
        messages,
        system: None,
        tools,
        tool_choice: None,
        stream: true,
        thinking,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    }
}

#[test]
fn builds_responses_shape_with_instructions_and_store_false() {
    let body = build_responses_request(
        &request(vec![InputMessage::user_text("hi")], None, None),
        "be helpful",
        true,
    );
    assert_eq!(body["model"], json!("gpt-5.6-sol"));
    assert_eq!(body["instructions"], json!("be helpful"));
    assert_eq!(body["store"], json!(false));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    // The Codex backend rejects any request-side output-token cap with a 400
    // `Unsupported parameter: max_output_tokens`, so neither the Responses name
    // nor the Chat Completions name may appear in the payload.
    assert!(
        body.get("max_output_tokens").is_none(),
        "Codex backend rejects max_output_tokens with 400; it must not be sent"
    );
    assert!(
        body.get("max_completion_tokens").is_none(),
        "Codex backend takes no output-token cap; max_completion_tokens is Chat Completions only"
    );
    assert!(
        body.get("max_tokens").is_none(),
        "Codex backend takes no output-token cap; max_tokens must not be sent"
    );
    assert_eq!(body["input"][0]["type"], json!("message"));
    assert_eq!(body["input"][0]["role"], json!("user"));
    assert_eq!(body["input"][0]["content"][0]["type"], json!("input_text"));
    assert_eq!(body["input"][0]["content"][0]["text"], json!("hi"));
    let key = body["prompt_cache_key"]
        .as_str()
        .expect("prompt cache key should be present");
    assert!(key.starts_with("zo-"));
    assert!(key.len() <= 64);
}

#[test]
fn gpt55_responses_request_uses_cache_key_without_unsupported_retention() {
    let body = build_responses_request(&request_with_model("gpt-5.5", None), "i", true);
    assert!(body.get("prompt_cache_key").is_some());
    assert!(body.get("prompt_cache_retention").is_none());
}

#[test]
fn prompt_cache_key_uses_stable_system_prefix_not_dynamic_tail() {
    let mut base = request_with_model("gpt-5.5", None);
    base.system = Some(vec![
        SystemBlock::text("stable identity"),
        SystemBlock::text("dynamic cwd /tmp/a"),
    ]);
    let mut changed_tail = base.clone();
    changed_tail.system = Some(vec![
        SystemBlock::text("stable identity"),
        SystemBlock::text("dynamic cwd /tmp/b"),
    ]);
    let mut changed_prefix = base.clone();
    changed_prefix.system = Some(vec![
        SystemBlock::text("different identity"),
        SystemBlock::text("dynamic cwd /tmp/a"),
    ]);

    let base_key = build_responses_request(&base, "i", true)["prompt_cache_key"].clone();
    let tail_key = build_responses_request(&changed_tail, "i", true)["prompt_cache_key"].clone();
    let prefix_key =
        build_responses_request(&changed_prefix, "i", true)["prompt_cache_key"].clone();

    assert_eq!(
        base_key, tail_key,
        "session-specific system tail must not fragment the GPT prompt cache key"
    );
    assert_ne!(
        base_key, prefix_key,
        "stable system prefix must still participate in the GPT prompt cache key"
    );
}

#[test]
fn prompt_cache_key_is_stable_within_session() {
    let first = request_with_model("gpt-5.6-sol", None);
    let mut next = first.clone();
    next.messages.push(InputMessage::user_text("next turn"));

    let first_key = build_responses_request_for_session(&first, "i", true, "session-a")
        ["prompt_cache_key"]
        .clone();
    let next_key = build_responses_request_for_session(&next, "i", true, "session-a")
        ["prompt_cache_key"]
        .clone();

    assert_eq!(first_key, next_key);
}

#[test]
fn prompt_cache_key_differs_across_sessions() {
    let request = request_with_model("gpt-5.6-sol", None);
    let first = build_responses_request_for_session(&request, "i", true, "session-a")
        ["prompt_cache_key"]
        .clone();
    let second = build_responses_request_for_session(&request, "i", true, "session-b")
        ["prompt_cache_key"]
        .clone();

    assert_ne!(first, second);
}

#[test]
fn empty_session_prompt_cache_key_matches_wrapper_key() {
    let request = request_with_model("gpt-5.6-sol", None);
    let wrapper_key = build_responses_request(&request, "i", true)["prompt_cache_key"].clone();
    let empty_session_key = build_responses_request_for_session(&request, "i", true, "")
        ["prompt_cache_key"]
        .clone();

    assert_eq!(empty_session_key, wrapper_key);
}

/// A rebuilt client (fresh random wire session id) with the same pinned cache
/// scope must resolve the same scope — model swaps and OAuth rotations
/// rebuild the client, and the provider cache key must not roll with them.
#[test]
fn cache_scope_pins_across_client_rebuilds() {
    let first = super::ChatGptBackendClient::new("t", None).with_cache_scope("session-1");
    let rebuilt = super::ChatGptBackendClient::new("t", None).with_cache_scope("session-1");
    assert_eq!(first.cache_scope(), "session-1");
    assert_eq!(first.cache_scope(), rebuilt.cache_scope());
    assert_eq!(first.pinned_cache_scope(), Some("session-1"));

    // Unpinned clients fall back to their per-instance wire session id.
    let bare = super::ChatGptBackendClient::new("t", None);
    assert_eq!(bare.pinned_cache_scope(), None);
    assert_eq!(bare.cache_scope(), bare.session_id);

    // An empty scope is "no scope", not a shared "" bucket.
    let empty = super::ChatGptBackendClient::new("t", None).with_cache_scope("");
    assert_eq!(empty.pinned_cache_scope(), None);
}

/// Two conversation streams sharing one session id (a fanout spawn re-stamps
/// its requests with the parent's session id) must land on distinct cache
/// keys, keyed by their distinct opening user messages — otherwise every
/// concurrent agent competes for one provider cache shard and evicts the
/// others' prefixes (observed live 07-20: sol cache reads pinned at the ~12k
/// shared system prefix across 400+ interleaved spawn requests).
#[test]
fn prompt_cache_key_differs_across_conversation_streams_in_one_session() {
    let main = request_with_model("gpt-5.6-sol", None);
    let mut spawn = main.clone();
    spawn.messages = vec![InputMessage::user_text("spawn task: audit crates/api")];

    let main_key = build_responses_request_for_session(&main, "i", true, "session-a")
        ["prompt_cache_key"]
        .clone();
    let spawn_key = build_responses_request_for_session(&spawn, "i", true, "session-a")
        ["prompt_cache_key"]
        .clone();

    assert_ne!(main_key, spawn_key);
}

#[test]
fn supported_responses_models_skip_unverified_prompt_cache_retention() {
    for model in [
        "gpt-5.5",
        "gpt-5.5-fast",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.3-codex-spark",
    ] {
        let body = build_responses_request(&request_with_model(model, None), "i", true);
        assert!(body.get("max_output_tokens").is_none(), "{model}");
        assert!(body.get("max_completion_tokens").is_none(), "{model}");
        assert!(body.get("prompt_cache_key").is_some(), "{model}");
        assert!(body.get("prompt_cache_retention").is_none(), "{model}");
    }
}

#[test]
fn preserves_user_image_blocks_in_responses_input() {
    let body = build_responses_request(
        &request(
            vec![InputMessage::user_with_images(
                "what is in this image?",
                vec![ImageSource {
                    kind: "base64".into(),
                    media_type: "image/png".into(),
                    data: "abc123".into(),
                }],
            )],
            None,
            None,
        ),
        "i",
        false,
    );

    assert_eq!(body["input"][0]["type"], json!("message"));
    assert_eq!(body["input"][0]["role"], json!("user"));
    assert_eq!(body["input"][0]["content"][0]["type"], json!("input_image"));
    assert_eq!(
        body["input"][0]["content"][0]["image_url"],
        json!("data:image/png;base64,abc123")
    );
    assert_eq!(body["input"][0]["content"][0]["detail"], json!("auto"));
    assert_eq!(body["input"][0]["content"][1]["type"], json!("input_text"));
    assert_eq!(
        body["input"][0]["content"][1]["text"],
        json!("what is in this image?")
    );
}

#[test]
fn degrades_user_image_blocks_for_vision_less_model() {
    let mut req = request(
        vec![InputMessage::user_with_images(
            "what is in this image?",
            vec![ImageSource {
                kind: "base64".into(),
                media_type: "image/png".into(),
                data: "abc123".into(),
            }],
        )],
        None,
        None,
    );
    req.model = "gpt-5.3-codex-spark".into();
    let body = build_responses_request(&req, "i", false);

    assert_eq!(body["input"][0]["type"], json!("message"));
    assert_eq!(body["input"][0]["role"], json!("user"));
    assert_eq!(body["input"][0]["content"][0]["type"], json!("input_text"));
    let placeholder = body["input"][0]["content"][0]["text"]
        .as_str()
        .expect("text content");
    assert!(
        placeholder.contains("[image omitted: image/png not sent because this model does not accept images]"),
        "expected placeholder, got: {placeholder}"
    );
    assert_eq!(body["input"][0]["content"][1]["type"], json!("input_text"));
    assert_eq!(
        body["input"][0]["content"][1]["text"],
        json!("what is in this image?")
    );
    // Ensure no input_image exists anywhere in input items
    let serialized = body["input"].to_string();
    assert!(
        !serialized.contains("input_image"),
        "vision-less model must not emit input_image: {serialized}"
    );
}

#[test]
fn translates_tool_use_and_result_to_function_call_items() {
    let assistant = InputMessage {
        role: "assistant".into(),
        content: vec![InputContentBlock::ToolUse {
            id: "call_1".into(),
            name: "read".into(),
            input: json!({ "path": "x" }),
                    cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: None,
    };
    let user_result = InputMessage {
        role: "user".into(),
        content: vec![InputContentBlock::ToolResult {
            tool_use_id: "call_1".into(),
            content: vec![crate::types::ToolResultContentBlock::Text {
                text: "data".into(),
            }],
            is_error: false,
                    cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: None,
    };
    let body = build_responses_request(
        &request(vec![assistant, user_result], None, None),
        "i",
        false,
    );
    assert_eq!(body["input"][0]["type"], json!("function_call"));
    assert_eq!(body["input"][0]["call_id"], json!("call_1"));
    assert_eq!(body["input"][0]["name"], json!("read"));
    assert_eq!(body["input"][0]["arguments"], json!("{\"path\":\"x\"}"));
    assert_eq!(body["input"][1]["type"], json!("function_call_output"));
    assert_eq!(body["input"][1]["call_id"], json!("call_1"));
    assert_eq!(body["input"][1]["output"], json!("data"));
}

// end-to-end responses-ordering test; body exceeds the 100-line lint threshold
#[allow(clippy::too_many_lines)]
#[test]
fn mixed_assistant_text_tool_calls_and_outputs_keep_responses_order() {
    cache_reasoning_for_call(
        "",
        "call_order_b",
        vec![json!({
            "type": "reasoning",
            "id": "rs_order_b",
            "encrypted_content": "OPAQUE-ORDER-B"
        })],
    );
    let body = build_responses_request(
        &request(
            vec![
                InputMessage {
                    role: "assistant".into(),
                    content: vec![
                        InputContentBlock::Text {
                            text: "I'll inspect then search.".into(),
                            cache_control: None,
                        },
                        InputContentBlock::ToolUse {
                            id: "call_order_a".into(),
                            name: "read_file".into(),
                            input: json!({"path": "a.rs"}),
                                                    cache_control: None,
                        },
                        InputContentBlock::ToolUse {
                            id: "call_order_b".into(),
                            name: "grep_search".into(),
                            input: json!({"pattern": "TODO"}),
                                                    cache_control: None,
                        },
                    ],
                    thought_signature: None,
                    reasoning_replay: None,
                },
                InputMessage {
                    role: "user".into(),
                    content: vec![InputContentBlock::ToolResult {
                        tool_use_id: "call_order_a".into(),
                        content: vec![crate::types::ToolResultContentBlock::Text {
                            text: "file text".into(),
                        }],
                        is_error: false,
                                            cache_control: None,
                    }],
                    thought_signature: None,
                    reasoning_replay: None,
                },
                InputMessage {
                    role: "user".into(),
                    content: vec![InputContentBlock::ToolResult {
                        tool_use_id: "call_order_b".into(),
                        content: vec![
                            crate::types::ToolResultContentBlock::Json {
                                value: json!({"matches": 2}),
                            },
                            crate::types::ToolResultContentBlock::Image {
                                source: ImageSource {
                                    kind: "base64".into(),
                                    media_type: "image/png".into(),
                                    data: "abc123".into(),
                                },
                            },
                        ],
                        is_error: true,
                                            cache_control: None,
                    }],
                    thought_signature: None,
                    reasoning_replay: None,
                },
            ],
            None,
            None,
        ),
        "i",
        false,
    );

    let input = body["input"].as_array().expect("input array");
    assert_eq!(input[0]["type"], json!("message"));
    assert_eq!(input[0]["role"], json!("assistant"));
    assert_eq!(
        input[0]["content"][0]["text"],
        json!("I'll inspect then search.")
    );
    assert_eq!(input[1]["type"], json!("function_call"));
    assert_eq!(input[1]["call_id"], json!("call_order_a"));
    assert_eq!(input[1]["name"], json!("read_file"));
    assert_eq!(input[1]["arguments"], json!("{\"path\":\"a.rs\"}"));
    assert_eq!(input[2]["type"], json!("reasoning"));
    assert_eq!(input[2]["encrypted_content"], json!("OPAQUE-ORDER-B"));
    assert_eq!(input[3]["type"], json!("function_call"));
    assert_eq!(input[3]["call_id"], json!("call_order_b"));
    assert_eq!(input[3]["arguments"], json!("{\"pattern\":\"TODO\"}"));
    assert_eq!(input[4]["type"], json!("function_call_output"));
    assert_eq!(input[4]["call_id"], json!("call_order_a"));
    assert_eq!(input[4]["output"], json!("file text"));
    assert_eq!(input[5]["type"], json!("function_call_output"));
    assert_eq!(input[5]["call_id"], json!("call_order_b"));
    assert_eq!(
        input[5]["output"],
        json!("{\"matches\":2}\n[image image/png]")
    );

    remove_reasoning_for_call("", "call_order_b");
}

#[test]
fn tools_and_reasoning_attached() {
    let body = build_responses_request(
        &request(
            vec![InputMessage::user_text("hi")],
            Some(vec![ToolDefinition {
                name: "read".into(),
                description: Some("Read a file".into()),
                input_schema: json!({ "type": "object" }),
            }]),
            Some(ThinkingConfig::enabled(8000)),
        ),
        "i",
        true,
    );
    assert_eq!(body["tools"][0]["type"], json!("function"));
    assert_eq!(body["tools"][0]["name"], json!("read"));
    assert_eq!(body["tool_choice"], json!("auto"));
    assert_eq!(body["reasoning"]["effort"], json!("high"));
    assert_eq!(body["reasoning"]["summary"], json!("auto"));
}

#[test]
fn forced_tool_choice_is_honored_in_responses_request() {
    let tools = Some(vec![ToolDefinition {
        name: "StructuredOutput".into(),
        description: Some("emit structured output".into()),
        input_schema: json!({ "type": "object" }),
    }]);
    // A workflow sub-agent forcing a named function must reach the Responses
    // API in its flat `{type,name}` form, not the silently-weakened "auto"
    // (BUG-R16).
    let mut req = request(vec![InputMessage::user_text("hi")], tools.clone(), None);
    req.tool_choice = Some(ToolChoice::Tool {
        name: "StructuredOutput".into(),
    });
    let body = build_responses_request(&req, "i", true);
    assert_eq!(
        body["tool_choice"],
        json!({ "type": "function", "name": "StructuredOutput" })
    );

    // `Any` maps to the Responses "required" mode; absent tool_choice stays "auto".
    let mut req_any = request(vec![InputMessage::user_text("hi")], tools, None);
    req_any.tool_choice = Some(ToolChoice::Any);
    let body_any = build_responses_request(&req_any, "i", true);
    assert_eq!(body_any["tool_choice"], json!("required"));
}

fn request_with_model(model: &str, budget: Option<u32>) -> MessageRequest {
    MessageRequest {
        model: model.into(),
        max_tokens: 1000,
        messages: vec![InputMessage::user_text("hi")],
        system: None,
        tools: None,
        tool_choice: None,
        stream: true,
        thinking: budget.map(ThinkingConfig::enabled),
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    }
}

#[test]
fn explicit_request_effort_drives_responses_reasoning_effort() {
    // zo_gpt's headless wire: build_message_request derives request.effort
    // from ZO_EFFORT and the Responses backend must serialize it as
    // reasoning.effort. GPT-5.6 added `max` to the enum, so the top tiers reach
    // the wire as `max` there and clamp to `xhigh` only on the families that
    // predate it. Distinct from the budget path the other tests cover.
    for (model, level, expected) in [
        ("gpt-5.5", crate::types::EffortLevel::Low, "low"),
        ("gpt-5.5", crate::types::EffortLevel::Medium, "medium"),
        ("gpt-5.5", crate::types::EffortLevel::High, "high"),
        ("gpt-5.5", crate::types::EffortLevel::Xhigh, "xhigh"),
        ("gpt-5.5", crate::types::EffortLevel::Max, "xhigh"),
        ("gpt-5.3-codex-spark", crate::types::EffortLevel::Max, "xhigh"),
        ("gpt-5.6-sol", crate::types::EffortLevel::Max, "max"),
        ("gpt-5.6-terra", crate::types::EffortLevel::Max, "max"),
        ("gpt-5.6-luna", crate::types::EffortLevel::Max, "max"),
        ("gpt-5.6-sol", crate::types::EffortLevel::Ultra, "max"),
        ("gpt-5.6-sol-2026-07-09", crate::types::EffortLevel::Ultra, "max"),
        ("gpt-5.6-terra@openai", crate::types::EffortLevel::Ultra, "max"),
        ("gpt-5.6-luna", crate::types::EffortLevel::Ultra, "max"),
        ("gpt-5.5", crate::types::EffortLevel::Ultra, "xhigh"),
    ] {
        let mut req = request_with_model(model, None);
        req.effort = Some(level);
        let body = build_responses_request(&req, "i", true);
        assert_eq!(
            body["reasoning"]["effort"],
            json!(expected),
            "{model} request.effort {level:?} should reach reasoning.effort"
        );
        assert!(body.get("max_output_tokens").is_none(), "{model}");
    }
}

/// The luna effort contract: `gpt-5.6-luna` always ships `xhigh` or above,
/// no matter what the band/dynamic/de-escalation pipeline resolved. Applied
/// last in `build_responses_request_for_session` (see `luna_effort_floor`).
#[test]
fn luna_effort_floors_at_xhigh_regardless_of_resolved_tier() {
    // Explicit low is lifted to the floor.
    let mut req = request_with_model("gpt-5.6-luna", None);
    req.effort = Some(crate::types::EffortLevel::Low);
    let body = build_responses_request(&req, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"), "explicit low");

    // A floor applies to the absent case too: other families now send no effort
    // when none was requested, but luna must not fall through to whatever its
    // adaptive default picks.
    let body = build_responses_request(&request_with_model("gpt-5.6-luna", None), "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"), "auto");

    // The empty-retry de-escalation must not step luna below the floor.
    let mut retry = request_with_model("gpt-5.6-luna", None);
    retry.system = Some(vec![crate::types::SystemBlock::text(
        "[zo:empty-response-retry] retry",
    )]);
    let body = build_responses_request(&retry, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"), "de-escalation");

    // Control: the floor is luna-scoped — sol leaves an unrequested effort to
    // the model.
    let body = build_responses_request(&request_with_model("gpt-5.6-sol", None), "i", true);
    assert_eq!(body["reasoning"].get("effort"), None, "sol control");
}

/// The Codex backend gates some models on the client User-Agent fingerprint:
/// `gpt-5.6-luna` answers 404 "Model not found" unless the UA carries the
/// `codex_cli_rs` product token (verified live 2026-07-13 — same token and
/// body, only the UA differing). Every request must therefore carry
/// [`super::USER_AGENT`].
#[test]
fn requests_carry_the_codex_user_agent_fingerprint() {
    let client = super::ChatGptBackendClient::new("token", Some("acct".to_string()));
    let request = client
        .apply_headers(
            reqwest::Client::new().post("https://chatgpt.com/backend-api/codex/responses"),
        )
        .build()
        .expect("request should build");
    let ua = request
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .expect("user-agent header present");
    assert!(
        ua.starts_with("codex_cli_rs/"),
        "the luna model gate requires the codex_cli_rs product token, got: {ua}"
    );
    assert_eq!(ua, super::USER_AGENT);
}

#[test]
fn explicit_ultra_stays_at_the_wire_ceiling_under_empty_response_retry() {
    let mut req = request_with_model("gpt-5.6-sol", None);
    req.effort = Some(crate::types::EffortLevel::Ultra);
    req.system = Some(vec![crate::types::SystemBlock::text(
        "[zo:empty-response-retry] retry",
    )]);
    let body = build_responses_request(&req, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("max"));
}

#[test]
fn gpt56_sol_projects_internal_top_efforts_to_the_supported_wire_ceiling() {
    // `max` is a real GPT-5.6 wire value, so the top tiers must reach it —
    // projecting them to `xhigh` made `/effort max` indistinguishable from
    // `/effort xhigh` on the family that introduced the rung. `ultra` is a
    // ChatGPT product mode, never a wire value, so it lands on `max` as well.
    for level in [
        crate::types::EffortLevel::Ultra,
        crate::types::EffortLevel::Max,
    ] {
        let mut req = request_with_model("gpt-5.6-sol", None);
        req.effort = Some(level);
        let body = build_responses_request(&req, "i", true);
        assert_eq!(body["reasoning"]["effort"], json!("max"), "{level:?}");
    }

    let long_ask = "word ".repeat(150);
    let smart = banded_request(
        "gpt-5.6-sol",
        &format!("please refactor this module. {long_ask}"),
    );
    let body = build_responses_request(&smart, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("max"), "Smart");
}

#[test]
fn model_normalizes_to_family_and_scales_effort_without_budget() {
    let body = build_responses_request(&request_with_model("gpt-5.5", None), "i", true);
    assert_eq!(body["model"], json!("gpt-5.5"));
    // No budget, not fast: no effort is sent at all — the model scales its own
    // reasoning — and the priority service tier is not requested.
    assert_eq!(body["reasoning"].get("effort"), None);
    assert!(body.get("service_tier").is_none());

    let dated = build_responses_request(&request_with_model("gpt-5.5-2026-04-23", None), "i", true);
    assert_eq!(dated["model"], json!("gpt-5.5"));
}

/// A tool that returns an image must reach the model as pixels.
///
/// `function_call_output.output` is a plain string, so images flatten to the
/// placeholder `[image image/png]` and the bytes are lost. The identical tool
/// result reaches Anthropic as a real image block, so a screenshot the model
/// requested arrived as seventeen characters here — it could not see its own
/// rendered output, which is exactly the loop a design task depends on.
#[test]
fn tool_result_images_are_reattached_as_input_images() {
    use crate::types::ToolResultContentBlock;

    let source = ImageSource {
        kind: "base64".into(),
        media_type: "image/png".into(),
        data: "aGVsbG8=".into(),
    };
    let request = MessageRequest {
        messages: vec![InputMessage {
            role: "user".into(),
            content: vec![InputContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: vec![
                    ToolResultContentBlock::Text { text: "screenshot captured".into() },
                    ToolResultContentBlock::Image { source: source.clone() },
                ],
                is_error: false,
                cache_control: None,
            }],
            thought_signature: None,
            reasoning_replay: None,
        }],
        ..request_with_model("gpt-5.6-sol", None)
    };

    let body = build_responses_request(&request, "i", false);
    let input = body["input"].as_array().expect("input items");

    let output = input
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .expect("tool result item");
    assert!(
        output["output"].as_str().expect("string output").contains("screenshot captured"),
        "text half of the result survives: {output}"
    );

    let image_item = input
        .iter()
        .find(|item| item["type"] == "message" && item["content"][0]["type"] == "input_image")
        .expect("image must be re-attached as its own input item");
    assert_eq!(image_item["role"], json!("user"));
    assert_eq!(
        image_item["content"][0]["image_url"],
        json!(crate::providers::image_data_url(&source)),
        "the actual pixels reach the model, not the placeholder text"
    );
}

#[test]
fn fast_variant_requests_priority_tier_without_touching_effort() {
    // gpt-5.5-fast is not a separate model: same gpt-5.5 family, no effort of
    // its own, but "/fast on" still requests the priority service tier for
    // ~1.5x faster serving — serving priority and reasoning effort are
    // independent controls.
    let body = build_responses_request(&request_with_model("gpt-5.5-fast", None), "i", true);
    assert_eq!(body["model"], json!("gpt-5.5"));
    assert_eq!(body["reasoning"].get("effort"), None);
    assert_eq!(body["service_tier"], json!("priority"));
}

#[test]
fn fast_mode_is_independent_of_reasoning_effort() {
    // Fast mode adds the priority tier without overriding a configured
    // reasoning budget — the two controls compose.
    let body = build_responses_request(&request_with_model("gpt-5.5-fast", Some(8_000)), "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("high"));
    assert_eq!(body["service_tier"], json!("priority"));
}

#[test]
fn gpt56_bracket_fast_suffix_requests_priority_tier_and_strips_to_bare_family() {
    // GPT-5.6's `/fast` toggle spells "fast" with a `[fast]` service-tier
    // suffix (not the legacy bare `-fast` alias) — this must ALSO reach the
    // wire as `service_tier: "priority"`, with the suffix stripped from the
    // model id sent to the backend (the bracket is a zo-side convention,
    // not something the Codex Responses API understands as part of a model id).
    let body = build_responses_request(&request_with_model("gpt-5.6-terra[fast]", None), "i", true);
    assert_eq!(body["model"], json!("gpt-5.6-terra"));
    assert_eq!(body["service_tier"], json!("priority"));

    let luna = build_responses_request(&request_with_model("gpt-5.6-luna[fast]", None), "i", true);
    assert_eq!(luna["model"], json!("gpt-5.6-luna"));
    assert_eq!(luna["service_tier"], json!("priority"));
}

#[test]
fn unregistered_dash_fast_suffix_is_not_treated_as_priority() {
    // Only the `[fast]` bracket convention (or the legacy bare `gpt-5.5-fast`
    // alias) triggers priority serving — an arbitrary `-fast`-suffixed id that
    // is not a registered alias must not be misread as one (mirrors the
    // existing Codex Spark `-fast` regression pin).
    let body = build_responses_request(&request_with_model("gpt-5.6-terra-fast", None), "i", true);
    assert!(body.get("service_tier").is_none());
}

#[test]
fn fast_variant_keeps_xhigh_and_clamps_max_like_legacy_gpt() {
    // `/fast` maps to priority serving (`service_tier`), not a lower reasoning
    // ceiling. It keeps Xhigh, but legacy GPT still clamps Max -> xhigh.
    for (level, expected) in [
        (crate::types::EffortLevel::Xhigh, "xhigh"),
        (crate::types::EffortLevel::Max, "xhigh"),
    ] {
        let mut req = request_with_model("gpt-5.5-fast", None);
        req.effort = Some(level);
        let body = build_responses_request(&req, "i", true);
        assert_eq!(
            body["reasoning"]["effort"],
            json!(expected),
            "fast variant should project {level:?} to {expected}"
        );
        assert_eq!(body["service_tier"], json!("priority"));
    }

    let body =
        build_responses_request(&request_with_model("gpt-5.5-fast", Some(30_000)), "i", true);
    assert_eq!(
        body["reasoning"]["effort"],
        json!("xhigh"),
        "fast variant must clamp a max-budget to xhigh"
    );

    let mut req = request_with_model("gpt-5.5", None);
    req.effort = Some(crate::types::EffortLevel::Xhigh);
    assert_eq!(
        build_responses_request(&req, "i", true)["reasoning"]["effort"],
        json!("xhigh"),
        "non-fast gpt-5.5 keeps xhigh"
    );
    assert_eq!(
        build_responses_request(&request_with_model("gpt-5.5", Some(30_000)), "i", true)["reasoning"]
            ["effort"],
        json!("xhigh"),
        "non-fast gpt-5.5 budget clamps max to xhigh"
    );
}

#[test]
fn codex_spark_is_not_fast_priority_and_keeps_top_effort() {
    // This is only a Codex regression pin: do not infer a production xhigh
    // ceiling from the `codex` token, and do not apply GPT `/fast` service tier.
    let mut req = request_with_model("gpt-5.3-codex-spark", None);
    req.effort = Some(crate::types::EffortLevel::Xhigh);
    let body = build_responses_request(&req, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"));
    assert!(body.get("service_tier").is_none());

    let body = build_responses_request(
        &request_with_model("gpt-5.3-codex-spark", Some(30_000)),
        "i",
        true,
    );
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"));
    assert!(body.get("service_tier").is_none());

    let body = build_responses_request(
        &request_with_model("gpt-5.3-codex-spark-fast", Some(30_000)),
        "i",
        true,
    );
    assert_eq!(body["model"], json!("gpt-5.3-codex-spark"));
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"));
    assert!(
        body.get("service_tier").is_none(),
        "do not infer priority from an unregistered Codex -fast suffix"
    );
}

#[test]
fn budget_drives_effort_for_non_fast() {
    let body = build_responses_request(&request_with_model("gpt-5.5", Some(8_000)), "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("high"));
}

#[test]
fn reasoning_effort_tiers() {
    assert_eq!(reasoning_effort(1_000), "low");
    assert_eq!(reasoning_effort(4_000), "medium");
    assert_eq!(reasoning_effort(10_000), "high");
    assert_eq!(reasoning_effort(16_000), "xhigh");
    assert_eq!(reasoning_effort(20_000), "xhigh");
    assert_eq!(reasoning_effort(20_001), "xhigh");
    assert_eq!(reasoning_effort(24_000), "xhigh");
    assert_eq!(reasoning_effort(30_000), "xhigh");
    assert_eq!(reasoning_effort(32_000), "xhigh");

    // `reasoning_effort` above is model-blind and stays conservative; told the
    // model, a max-tier budget reaches GPT-5.6's real `max` rung.
    let mut req = request_with_model("gpt-5.6-sol", Some(24_000));
    assert_eq!(build_responses_request(&req, "i", true)["reasoning"]["effort"], json!("max"));
    req.model = "gpt-5.6-terra".to_string();
    assert_eq!(build_responses_request(&req, "i", true)["reasoning"]["effort"], json!("max"));
    req.model = "gpt-5.6-luna".to_string();
    assert_eq!(build_responses_request(&req, "i", true)["reasoning"]["effort"], json!("max"));
}

#[test]
fn terminal_stream_recovery_deliberately_deescalates_ultra() {
    let mut req = request_with_model("gpt-5.6-sol", None);
    req.effort = Some(crate::types::EffortLevel::Ultra);
    let recovered = deescalated_recovery_request(&req);
    assert!(!recovered.stream);
    assert_eq!(recovered.effort, Some(crate::types::EffortLevel::High));
    assert!(recovered.thinking.is_none());
}

#[test]
fn terminal_stream_recovery_clears_the_band_ceiling_so_it_cannot_re_escalate() {
    // A banded (Smart-mode) request's floor gets forced down to a static
    // High above; if `effort_band_ceiling` survived the clone, the wire seam
    // would run `resolve_effort_band` again and could re-escalate a heavy
    // turn right back past the deliberate de-escalation.
    let req = banded_request("gpt-5.6-sol", "please refactor this module");
    let recovered = deescalated_recovery_request(&req);
    assert_eq!(recovered.effort, Some(crate::types::EffortLevel::High));
    assert_eq!(recovered.effort_band_ceiling, None);
    let body = build_responses_request(&recovered, "i", false);
    assert_eq!(body["reasoning"]["effort"], json!("high"));
}

/// With no explicit tier, nothing is sent and the model picks its own.
///
/// A host-side ladder used to fill this in from a keyword table and message
/// length, which meant a short request went out at `low` however large the work
/// behind it was. There is no threshold to test now — the contract is that the
/// field is absent, whatever the request looks like.
#[test]
fn auto_effort_sends_no_reasoning_effort() {
    for ask in [
        "hi",
        "build a 3D physics sim of an anvil crushing a car",
        "아케이드 미니게임 하나 만들어줘",
        "please refactor this module",
    ] {
        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.5"] {
            let request = MessageRequest {
                messages: vec![InputMessage::user_text(ask)],
                ..request_with_model(model, None)
            };
            let body = build_responses_request(&request, "i", false);
            assert_eq!(
                body["reasoning"].get("effort"),
                None,
                "{model} must leave effort to the model for {ask:?}: {body}"
            );
        }
    }
}

/// An explicit tier still wins — this is what `/effort` is for.
#[test]
fn explicit_effort_still_reaches_the_wire() {
    let mut request = MessageRequest {
        messages: vec![InputMessage::user_text("hi")],
        ..request_with_model("gpt-5.6-sol", None)
    };
    request.effort = Some(EffortLevel::High);
    let body = build_responses_request(&request, "i", false);
    assert_eq!(body["reasoning"]["effort"], json!("high"));
}

fn banded_request(model: &str, text: &str) -> MessageRequest {
    MessageRequest {
        model: model.into(),
        max_tokens: 1000,
        messages: vec![InputMessage::user_text(text)],
        system: None,
        tools: None,
        tool_choice: None,
        stream: true,
        thinking: None,
        output_config: None,
        effort: Some(crate::types::EffortLevel::Xhigh),
        effort_band_ceiling: Some(crate::types::EffortLevel::Max),
    }
}

#[test]
fn banded_request_never_leaks_internal_top_rungs_after_gpt_projection() {
    // Smart mode: MessageRequest.effort carries the floor (Xhigh) and
    // effort_band_ceiling carries the ceiling (Max) — build_responses_request
    // resolves the band BEFORE gpt_for_model. The band is what makes the wire
    // value move: a trivial ask sits at the floor and a heavy one escalates,
    // which is only observable now that the top rung is no longer flattened
    // into `xhigh`.
    let trivial = banded_request("gpt-5.6-sol", "hi");
    let body = build_responses_request(&trivial, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"));

    let one_signal = banded_request("gpt-5.6-sol", "please refactor this module");
    let body = build_responses_request(&one_signal, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("max"));

    let long_ask = "word ".repeat(150);
    let two_signal = banded_request(
        "gpt-5.6-sol",
        &format!("please refactor this module. {long_ask}"),
    );
    let body = build_responses_request(&two_signal, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("max"));

    // Luna's own ceiling is Max too, so the heavy rung lands identically.
    let two_signal_luna = banded_request(
        "gpt-5.6-luna",
        &format!("please refactor this module. {long_ask}"),
    );
    let body = build_responses_request(&two_signal_luna, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("max"));
}

#[test]
fn banded_request_stays_explicit_top_effort_protected_under_empty_response_pressure() {
    // The floor is Xhigh, so explicit_top_effort still fires for a banded
    // request (it IS a user top-mode contract) — an empty-response retry must
    // not de-escalate a band pick, even though it resolves per-request.
    let mut heavy = banded_request("gpt-5.6-sol", "please refactor this module");
    heavy.system = Some(crate::types::system_from_string(
        "[zo:empty-response-retry] previous attempt produced no output",
    ));
    let body = build_responses_request(&heavy, "i", true);
    assert_eq!(
        body["reasoning"]["effort"],
        json!("max"),
        "band pick must not be de-escalated by empty-response retry pressure"
    );
}

/// The live shape: the runtime persists its reminders as a trailing `System`
/// transcript message, which lowers to a `user`-role wire message — NOT to a
/// system block.
///
/// Scanning only `request.system` therefore found nothing on every real
/// session, so the retry went back out at the identical effort, walking the
/// identical path, and ended empty again. The existing coverage all builds the
/// system-block shape the runtime stopped producing.
#[test]
fn empty_retry_pressure_is_seen_when_the_marker_rides_the_transcript() {
    let mut retry = request_with_model("gpt-5.6-sol", None);
    retry.effort = Some(crate::types::EffortLevel::High);
    retry.messages = vec![
        InputMessage::user_text("do the thing"),
        InputMessage::user_text(
            "[zo:empty-response-retry] <system-reminder>The previous assistant response \
             ended with no text or tool call.</system-reminder>",
        ),
    ];
    let body = build_responses_request(&retry, "i", true);
    assert_eq!(
        body["reasoning"]["effort"],
        json!("medium"),
        "high must step down once when the previous attempt produced nothing"
    );

    // The continuation marker is the other half of the same signal.
    retry.messages = vec![InputMessage::user_text(
        "[zo:empty-response-continuation] <system-reminder>continue</system-reminder>",
    )];
    let body = build_responses_request(&retry, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("medium"));
}

/// A transcript that merely QUOTES a marker must not trigger the retry
/// de-escalation — an agent working on this very file has the literal in its
/// context, and a loose scan would let a conversation about the retry system
/// drive the retry system.
#[test]
fn a_quoted_marker_in_the_history_is_not_empty_response_pressure() {
    let mut quoted = request_with_model("gpt-5.6-sol", None);
    quoted.effort = Some(crate::types::EffortLevel::High);
    quoted.messages = vec![
        // Mid-text mention, and in an older message rather than the newest.
        InputMessage::user_text("why does [zo:empty-response-retry] never fire?"),
        InputMessage::user_text("look at empty_response_pressure"),
    ];
    let body = build_responses_request(&quoted, "i", true);
    assert_eq!(
        body["reasoning"]["effort"],
        json!("high"),
        "an ordinary question about the marker is not the marker"
    );
}

/// The half of the ladder that could never run. `ReasoningRequest::Auto`
/// resolves to `None` — no effort field at all — so the `Some(effort)` arm
/// could not reach it and the retry re-sent a request byte-identical to the one
/// that came back empty. An Auto turn is exactly the case where the model chose
/// its own scale, spent the window on it, and produced nothing; leaving it
/// untouched guarantees the retry repeats the attempt.
#[test]
fn an_auto_request_retries_at_a_named_rung_instead_of_repeating_itself() {
    use super::empty_retry_effort;

    assert_eq!(empty_retry_effort(None, false), Some("medium"));
    // A named rung still steps down once.
    assert_eq!(empty_retry_effort(Some("max"), false), Some("high"));
    // A user-selected top-effort contract stays top-tier.
    assert_eq!(empty_retry_effort(Some("max"), true), Some("max"));
    // Already at the floor: nothing to step, and nothing pretends otherwise.
    assert_eq!(empty_retry_effort(Some("low"), false), Some("low"));
}

#[test]
fn de_escalated_effort_steps_ultra_down_to_max_not_all_the_way_to_low() {
    // Defensive compatibility for any legacy/raw value reaching the helper.
    // Current GPT request builders project internal Ultra/Max to xhigh before
    // this point, so neither unsupported token is emitted on new requests.
    assert_eq!(de_escalated_effort("ultra"), "max");
    assert_eq!(de_escalated_effort("max"), "high");
    assert_eq!(de_escalated_effort("xhigh"), "medium");
    assert_eq!(de_escalated_effort("high"), "medium");
    assert_eq!(de_escalated_effort("medium"), "low");
}

#[test]
fn sse_parser_extracts_data_frames_and_skips_done() {
    let mut parser = ResponsesSseParser::new();
    let events = parser
        .push(
            b"event: response.created\ndata: {\"type\":\"response.created\"}\n\ndata: [DONE]\n\n",
        )
        .expect("well-formed frames parse");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], json!("response.created"));
}

#[test]
fn ingest_maps_text_item_to_block_events() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string());
    assert!(matches!(
        state.ingest(&json!({"type":"response.created","response":{"id":"resp_1"}}))[0],
        StreamEvent::MessageStart(_)
    ));
    assert!(matches!(
        state.ingest(&json!({
            "type":"response.output_item.added","output_index":0,
            "item":{"type":"message","role":"assistant"}
        }))[0],
        StreamEvent::ContentBlockStart(_)
    ));
    let delta = state.ingest(&json!({
        "type":"response.output_text.delta","output_index":0,"delta":"Hi"
    }));
    match &delta[0] {
        StreamEvent::ContentBlockDelta(event) => assert!(matches!(
            &event.delta,
            ContentBlockDelta::TextDelta { text } if text == "Hi"
        )),
        other => panic!("expected text delta, got {other:?}"),
    }
    assert!(matches!(
        state.ingest(&json!({"type":"response.output_item.done","output_index":0,"item":{}}))[0],
        StreamEvent::ContentBlockStop(_)
    ));
}

/// Codex CLI parity: reasoning items completed by `output_item.done` are
/// cached under the following `function_call`'s `call_id`, and the next
/// request's input replays them (encrypted content intact) immediately
/// before that call's `function_call` item. Without the replay, the
/// stateless Codex backend loses all reasoning continuity across turns —
/// the measured gpt-5.5 quality gap versus codex desktop.
#[test]
fn reasoning_items_round_trip_into_next_request() {
    let session_id = "session-rt1";
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string()).with_session_id(session_id);
    // A reasoning item completes, then the tool call it preceded.
    let _ = state.ingest(&json!({
        "type":"response.output_item.added","output_index":0,
        "item":{"type":"reasoning"}
    }));
    let _ = state.ingest(&json!({
        "type":"response.output_item.done","output_index":0,
        "item":{"type":"reasoning","id":"rs_1",
                "encrypted_content":"OPAQUE-BLOB",
                "summary":[]}
    }));
    let _ = state.ingest(&json!({
        "type":"response.output_item.added","output_index":1,
        "item":{"type":"function_call","call_id":"call_rt1","name":"read"}
    }));
    let _ = state.ingest(&json!({
        "type":"response.output_item.done","output_index":1,
        "item":{"type":"function_call","call_id":"call_rt1","name":"read",
                "arguments":"{}"}
    }));

    // The cache holds the full reasoning item for that call id.
    let cached =
        reasoning_for_call(session_id, "call_rt1").expect("reasoning cached for the call");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0]["encrypted_content"], json!("OPAQUE-BLOB"));

    // Rebuilding the request from zo's provider-agnostic history (which
    // only carries the ToolUse) replays the reasoning item right before
    // the function_call input item.
    let request = request(
        vec![
            InputMessage::user_text("do the thing"),
            InputMessage {
                role: "assistant".into(),
                content: vec![InputContentBlock::ToolUse {
                    id: "call_rt1".into(),
                    name: "read".into(),
                    input: json!({"path":"x"}),
                                    cache_control: None,
                }],
                thought_signature: None,
                reasoning_replay: None,
            },
            InputMessage {
                role: "user".into(),
                content: vec![InputContentBlock::ToolResult {
                    tool_use_id: "call_rt1".into(),
                    content: vec![],
                    is_error: false,
                                    cache_control: None,
                }],
                thought_signature: None,
                reasoning_replay: None,
            },
        ],
        None,
        None,
    );
    let body = build_responses_request_for_session(&request, "i", true, session_id);
    let input = body["input"].as_array().expect("input array");
    let reasoning_pos = input
        .iter()
        .position(|item| item["type"] == json!("reasoning"))
        .expect("reasoning item replayed into input");
    let call_pos = input
        .iter()
        .position(|item| item["type"] == json!("function_call"))
        .expect("function_call present");
    assert_eq!(
        input[reasoning_pos]["encrypted_content"],
        json!("OPAQUE-BLOB")
    );
    assert!(
        reasoning_pos < call_pos,
        "reasoning must precede its function_call"
    );

    // An unknown call id replays nothing.
    assert!(reasoning_for_call(session_id, "call_unknown").is_none());
    // Re-recording the same id replaces, not duplicates.
    cache_reasoning_for_call(
        session_id,
        "call_rt1",
        vec![json!({"type":"reasoning","id":"rs_2"})],
    );
    let replaced = reasoning_for_call(session_id, "call_rt1").expect("still cached");
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0]["id"], json!("rs_2"));
}

/// Root-fix determinism: a message-attached `reasoning_replay` payload makes
/// `build_responses_request` reproducible regardless of the process-wide
/// cache's state — two calls with the same history produce byte-identical
/// requests, and growing an *unrelated* session's cache by 300 entries in
/// between changes nothing (the cache fallback is never consulted when the
/// attached field is present).
#[test]
fn deterministic_replay_from_attached_field_is_unaffected_by_other_session_cache_growth() {
    let assistant = InputMessage {
        role: "assistant".into(),
        content: vec![InputContentBlock::ToolUse {
            id: "call_det".into(),
            name: "read".into(),
            input: json!({"path": "x"}),
                    cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: Some(json!([
            {"call_id": "call_det", "items": [
                {"type": "reasoning", "id": "rs_det", "encrypted_content": "OPAQUE-DET"}
            ]}
        ])),
    };
    let req = request(
        vec![InputMessage::user_text("go"), assistant],
        None,
        None,
    );

    let body1 = build_responses_request_for_session(&req, "i", true, "session-det");
    let body2 = build_responses_request_for_session(&req, "i", true, "session-det");
    assert_eq!(body1, body2);
    assert_eq!(
        body1.to_string(),
        body2.to_string(),
        "identical history must produce byte-identical request JSON"
    );

    // Grow a completely unrelated session's cache well past its own cap.
    for i in 0..300 {
        cache_reasoning_for_call(
            "session-other",
            &format!("call_other_{i}"),
            vec![json!({"type": "reasoning", "id": format!("rs_other_{i}")})],
        );
    }

    let body3 = build_responses_request_for_session(&req, "i", true, "session-det");
    assert_eq!(
        body1.to_string(),
        body3.to_string(),
        "unrelated session cache growth must not perturb a request using the attached field"
    );
}

/// Build a history of `n` tool-calling assistant messages (each followed by
/// its tool result), every one carrying an attached `reasoning_replay`
/// payload, and return the wire `input` array.
fn wire_input_with_tool_calls(n: usize) -> Vec<serde_json::Value> {
    let mut messages = vec![InputMessage::user_text("go")];
    for i in 0..n {
        let call_id = format!("call_{i}");
        messages.push(InputMessage {
            role: "assistant".into(),
            content: vec![InputContentBlock::ToolUse {
                id: call_id.clone(),
                name: "read".into(),
                input: json!({}),
                cache_control: None,
            }],
            thought_signature: None,
            reasoning_replay: Some(json!([
                {"call_id": call_id, "items": [
                    {"type": "reasoning", "id": format!("rs_{i}")}
                ]}
            ])),
        });
        messages.push(InputMessage {
            role: "user".into(),
            content: vec![InputContentBlock::ToolResult {
                tool_use_id: call_id,
                content: vec![],
                is_error: false,
                cache_control: None,
            }],
            thought_signature: None,
            reasoning_replay: None,
        });
    }
    let req = request(messages, None, None);
    let body = build_responses_request(&req, "i", true);
    body["input"].as_array().expect("input array").clone()
}

/// Append-stability regression: the 17th tool call must NOT evict the 1st
/// tool call's reasoning from the wire. The original "most recent 16" window
/// slid on every appended tool call, mutating mid-history on every request —
/// which broke the provider prefix cache right after the system prompt and
/// re-billed the whole transcript per call (observed live: cache reads pinned
/// at ~10k while input grew past 200k).
#[test]
fn reasoning_replay_seventeenth_tool_call_keeps_the_first_replayed() {
    let input = wire_input_with_tool_calls(17);
    for i in 0..17 {
        assert!(
            input.iter().any(|item| item["id"] == json!(format!("rs_{i}"))),
            "every reasoning item must be replayed, missing rs_{i}: {input:?}"
        );
    }
}

/// Replay is append-only at any depth: 48 tool calls (past the old stride-16
/// staircase's two anchor jumps) must all keep their reasoning items, each
/// immediately before its `function_call`. The staircase's anchor advance
/// dropped the oldest stride's items in one step — a mid-history mutation
/// that re-billed the whole post-anchor suffix on every jump.
#[test]
fn reasoning_replay_replays_every_tool_call_at_any_depth() {
    let input = wire_input_with_tool_calls(48);
    for i in 0..48 {
        let call_id = json!(format!("call_{i}"));
        let call_pos = input
            .iter()
            .position(|item| item["call_id"] == call_id)
            .unwrap_or_else(|| panic!("call_{i} function_call present"));
        assert_eq!(
            input[call_pos - 1]["type"],
            json!("reasoning"),
            "call_{i} should be preceded by its reasoning item: {input:?}"
        );
        assert_eq!(input[call_pos - 1]["id"], json!(format!("rs_{i}")));
    }
}

/// The prefix-cache safety property stated at wire level: growing the history
/// by one tool call appends input items and changes nothing before them, so
/// the provider's prefix cache stays valid across every request of a
/// conversation. This is exactly the property the stride-16 staircase anchor
/// violated once per stride.
#[test]
fn reasoning_replay_wire_input_is_append_only_as_history_grows() {
    for n in [1, 15, 16, 17, 31, 32, 47, 48] {
        let shorter = wire_input_with_tool_calls(n);
        let longer = wire_input_with_tool_calls(n + 1);
        assert!(
            longer.len() > shorter.len(),
            "growing the history must append items (n={n})"
        );
        assert_eq!(
            longer[..shorter.len()],
            shorter[..],
            "history growth must never mutate earlier wire input (n={n})"
        );
    }
}

/// Session scoping (Stage A defense line): a session's cached reasoning
/// entry survives a different session pushing 300 entries of its own — the
/// old process-wide FIFO cache would have evicted it.
#[test]
fn session_scoped_cache_isolates_unrelated_sessions() {
    cache_reasoning_for_call(
        "session-a",
        "call_a",
        vec![json!({"type": "reasoning", "id": "rs_a"})],
    );

    for i in 0..300 {
        cache_reasoning_for_call(
            "session-b",
            &format!("call_b_{i}"),
            vec![json!({"type": "reasoning", "id": format!("rs_b_{i}")})],
        );
    }

    let cached = reasoning_for_call("session-a", "call_a")
        .expect("session A's entry must survive session B's 300 pushes");
    assert_eq!(cached[0]["id"], json!("rs_a"));
}

#[test]
fn reasoning_summary_parts_get_a_paragraph_separator() {
    use crate::types::{ContentBlockDelta, StreamEvent};

    let thinking_text = |events: &[StreamEvent]| -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ContentBlockDelta(delta) => match &delta.delta {
                    ContentBlockDelta::ThinkingDelta { thinking } => Some(thinking.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    };

    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    let _ = state.ingest(&json!({
        "type":"response.output_item.added","output_index":0,
        "item":{"type":"reasoning"}
    }));
    // Part 0 opens: no separator (nothing precedes it).
    let first_part = state.ingest(&json!({
        "type":"response.reasoning_summary_part.added","output_index":0,"summary_index":0
    }));
    assert!(thinking_text(&first_part).is_empty());
    let _ = state.ingest(&json!({
        "type":"response.reasoning_summary_text.delta","output_index":0,
        "delta":"First topic."
    }));
    // Part 1 opens: OpenAI sends no separator between summary parts, so the
    // adapter must inject a paragraph break — otherwise parts render as one
    // run-on paragraph and the TUI's rolling title freezes on part 0.
    let second_part = state.ingest(&json!({
        "type":"response.reasoning_summary_part.added","output_index":0,"summary_index":1
    }));
    assert_eq!(thinking_text(&second_part), vec!["\n\n".to_string()]);
    // A boundary must never phantom-start a reasoning block by itself.
    let mut fresh = ResponsesStreamState::new("gpt-5.5".to_string());
    let orphan = fresh.ingest(&json!({
        "type":"response.reasoning_summary_part.added","output_index":3,"summary_index":1
    }));
    assert!(orphan.is_empty());
}

#[test]
fn ingest_maps_function_call_and_args_delta() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string());
    let added = state.ingest(&json!({
        "type":"response.output_item.added","output_index":1,
        "item":{"type":"function_call","call_id":"call_9","name":"read"}
    }));
    match &added[0] {
        StreamEvent::ContentBlockStart(event) => match &event.content_block {
            OutputContentBlock::ToolUse { id, name, .. } => {
                assert_eq!(id, "call_9");
                assert_eq!(name, "read");
            }
            other => panic!("expected tool use, got {other:?}"),
        },
        other => panic!("expected block start, got {other:?}"),
    }
    let args = state.ingest(&json!({
        "type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"p\":1}"
    }));
    match &args[0] {
        StreamEvent::ContentBlockDelta(event) => assert!(matches!(
            &event.delta,
            ContentBlockDelta::InputJsonDelta { partial_json } if partial_json == "{\"p\":1}"
        )),
        other => panic!("expected json delta, got {other:?}"),
    }
}

#[test]
fn done_events_supply_text_when_deltas_are_absent() {
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    let events = state.ingest(&json!({
        "type":"response.output_text.done",
        "output_index":0,
        "text":"final text only"
    }));
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], StreamEvent::ContentBlockStart(_)));
    match &events[1] {
        StreamEvent::ContentBlockDelta(event) => assert!(matches!(
            &event.delta,
            ContentBlockDelta::TextDelta { text } if text == "final text only"
        )),
        other => panic!("expected text delta, got {other:?}"),
    }

    let duplicate = state.ingest(&json!({
        "type":"response.content_part.done",
        "output_index":0,
        "part":{"type":"text","text":"final text only"}
    }));
    assert!(
        duplicate.is_empty(),
        "done payloads must not duplicate prior text deltas"
    );
}

#[test]
fn function_call_arguments_done_supplies_args_without_delta() {
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    let added = state.ingest(&json!({
        "type":"response.output_item.added","output_index":2,
        "item":{"type":"function_call","call_id":"call_done","name":"read"}
    }));
    assert!(matches!(added[0], StreamEvent::ContentBlockStart(_)));

    let args = state.ingest(&json!({
        "type":"response.function_call_arguments.done",
        "output_index":2,
        "call_id":"call_done",
        "name":"read",
        "arguments":"{\"path\":\"x\"}"
    }));
    assert_eq!(args.len(), 1);
    match &args[0] {
        StreamEvent::ContentBlockDelta(event) => assert!(matches!(
            &event.delta,
            ContentBlockDelta::InputJsonDelta { partial_json } if partial_json == "{\"path\":\"x\"}"
        )),
        other => panic!("expected input json delta, got {other:?}"),
    }

    let duplicate = state.ingest(&json!({
        "type":"response.output_item.done","output_index":2,
        "item":{"type":"function_call","call_id":"call_done","name":"read",
                "arguments":"{\"path\":\"x\"}"}
    }));
    assert_eq!(duplicate.len(), 1);
    assert!(matches!(
        duplicate[0],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 2 })
    ));
}

#[test]
fn output_item_done_supplies_final_payloads_without_prior_done_events() {
    let mut text_state = ResponsesStreamState::new("gpt-5.5".to_string());
    let text = text_state.ingest(&json!({
        "type":"response.output_item.done","output_index":0,
        "item":{"type":"message","content":[
            {"type":"output_text","text":"hello"},
            {"type":"output_text","text":" world"}
        ]}
    }));
    assert_eq!(text.len(), 3);
    assert!(matches!(text[0], StreamEvent::ContentBlockStart(_)));
    match &text[1] {
        StreamEvent::ContentBlockDelta(event) => assert!(matches!(
            &event.delta,
            ContentBlockDelta::TextDelta { text } if text == "hello world"
        )),
        other => panic!("expected text delta, got {other:?}"),
    }
    assert!(matches!(
        text[2],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
    ));

    let mut tool_state = ResponsesStreamState::new("gpt-5.5".to_string());
    let tool = tool_state.ingest(&json!({
        "type":"response.output_item.done","output_index":1,
        "item":{"type":"function_call","call_id":"call_item","name":"read",
                "arguments":"{\"path\":\"z\"}"}
    }));
    assert_eq!(tool.len(), 3);
    assert!(matches!(tool[0], StreamEvent::ContentBlockStart(_)));
    match &tool[1] {
        StreamEvent::ContentBlockDelta(event) => assert!(matches!(
            &event.delta,
            ContentBlockDelta::InputJsonDelta { partial_json } if partial_json == "{\"path\":\"z\"}"
        )),
        other => panic!("expected input json delta, got {other:?}"),
    }
    assert!(matches!(
        tool[2],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 1 })
    ));
}

#[test]
fn completed_event_supplies_output_snapshot_without_prior_deltas() {
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    let events = state.ingest(&json!({
        "type":"response.completed",
        "response":{
            "output":[
                {"type":"message","content":[
                    {"type":"output_text","text":"from completed"}
                ]},
                {"type":"function_call","call_id":"call_done","name":"read",
                 "arguments":"{\"path\":\"x\"}"}
            ],
            "usage":{"input_tokens":3,"output_tokens":4}
        }
    }));

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                delta: ContentBlockDelta::TextDelta { text },
                ..
            }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "from completed");

    let args: String = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                delta: ContentBlockDelta::InputJsonDelta { partial_json },
                ..
            }) => Some(partial_json.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(args, "{\"path\":\"x\"}");
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 1 })
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageStop(_)))
    );
}

#[test]
fn completed_event_does_not_duplicate_prior_text_delta() {
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    let mut events = Vec::new();
    events.extend(state.ingest(&json!({
        "type":"response.output_item.added",
        "output_index":0,
        "item":{"type":"message"}
    })));
    events.extend(state.ingest(&json!({
        "type":"response.output_text.delta",
        "output_index":0,
        "delta":"already streamed"
    })));
    events.extend(state.ingest(&json!({
        "type":"response.output_item.done",
        "output_index":0,
        "item":{"type":"message","content":[
            {"type":"output_text","text":"already streamed"}
        ]}
    })));
    events.extend(state.ingest(&json!({
        "type":"response.completed",
        "response":{
            "output":[{"type":"message","content":[
                {"type":"output_text","text":"already streamed"}
            ]}],
            "usage":{"input_tokens":1,"output_tokens":1}
        }
    })));

    let text_deltas = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                    delta: ContentBlockDelta::TextDelta { .. },
                    ..
                })
            )
        })
        .count();
    assert_eq!(text_deltas, 1);
}

#[test]
fn completed_emits_usage_then_stop() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string());
    let events = state.ingest(&json!({
        "type":"response.completed",
        "response":{"usage":{"input_tokens":12,"output_tokens":7,
            "input_tokens_details":{"cached_tokens":5}}}
    }));
    assert_eq!(events.len(), 2);
    match &events[0] {
        StreamEvent::MessageDelta(event) => {
            assert_eq!(event.usage.input_tokens, 7);
            assert_eq!(event.usage.cache_read_input_tokens, 5);
            assert_eq!(event.usage.output_tokens, 7);
            assert_eq!(event.delta.stop_reason.as_deref(), Some("end_turn"));
        }
        other => panic!("expected message delta, got {other:?}"),
    }
    assert!(matches!(events[1], StreamEvent::MessageStop(_)));
    assert!(
        state
            .ingest(&json!({"type":"response.completed","response":{}}))
            .is_empty()
    );
}

#[test]
fn completed_closes_open_text_item_before_message_stop() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string());
    assert!(matches!(
        state.ingest(&json!({
            "type":"response.output_item.added","output_index":0,
            "item":{"type":"message","role":"assistant"}
        }))[0],
        StreamEvent::ContentBlockStart(_)
    ));
    assert!(matches!(
        state.ingest(&json!({
            "type":"response.output_text.delta","output_index":0,"delta":"tail"
        }))[0],
        StreamEvent::ContentBlockDelta(_)
    ));

    let events = state.ingest(&json!({
        "type":"response.completed",
        "response":{"usage":{"input_tokens":1,"output_tokens":1}}
    }));
    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[0],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
    ));
    assert!(matches!(events[1], StreamEvent::MessageDelta(_)));
    assert!(matches!(events[2], StreamEvent::MessageStop(_)));
}

#[test]
fn completed_closes_text_delta_even_when_item_done_is_missing() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string());
    let delta = state.ingest(&json!({
        "type":"response.output_text.delta","output_index":0,"delta":"early"
    }));
    assert!(matches!(delta[0], StreamEvent::ContentBlockStart(_)));
    assert!(matches!(delta[1], StreamEvent::ContentBlockDelta(_)));

    let events = state.ingest(&json!({
        "type":"response.completed",
        "response":{"usage":{"input_tokens":1,"output_tokens":1}}
    }));
    assert!(matches!(
        events[0],
        StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: 0 })
    ));
    assert!(matches!(events[1], StreamEvent::MessageDelta(_)));
    assert!(matches!(events[2], StreamEvent::MessageStop(_)));
}

#[test]
fn completed_ignores_late_item_done() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string());
    assert_eq!(
        state
            .ingest(&json!({
                "type":"response.output_text.delta","output_index":0,"delta":"tail"
            }))
            .len(),
        2
    );
    assert_eq!(
        state
            .ingest(&json!({"type":"response.completed","response":{}}))
            .len(),
        3
    );
    assert!(
        state
            .ingest(&json!({"type":"response.output_item.done","output_index":0,"item":{}}))
            .is_empty()
    );
}

#[test]
fn text_delta_before_item_added_synthesizes_single_start() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string());
    let delta = state.ingest(&json!({
        "type":"response.output_text.delta","output_index":0,"delta":"early"
    }));
    assert!(matches!(delta[0], StreamEvent::ContentBlockStart(_)));
    assert!(matches!(delta[1], StreamEvent::ContentBlockDelta(_)));

    assert!(
        state
            .ingest(&json!({
                "type":"response.output_item.added","output_index":0,
                "item":{"type":"message","role":"assistant"}
            }))
            .is_empty()
    );
}

#[test]
fn client_constructs_and_joins_system_instructions() {
    let _client = super::ChatGptBackendClient::new("token", Some("acc_1".to_string()))
        .with_base_url("http://localhost/responses");
    let mut request = request_with_model("gpt-5.5", None);
    request.system = Some(crate::types::system_from_string("be terse"));
    let instructions = super::ChatGptBackendClient::instructions(&request);
    // The zo system body is preserved …
    assert!(
        instructions.contains("be terse"),
        "system body preserved: {instructions}"
    );
    // … under an explicit OpenAI model identity that overrides zo's
    // hardcoded Claude identity so gpt-5.5 doesn't introduce itself as Claude.
    assert!(
        instructions.contains("gpt-5.5") && instructions.contains("OpenAI"),
        "identity override present: {instructions}"
    );
}

#[test]
fn instructions_empty_system_stays_empty() {
    // No system blocks → no identity preamble, no batching contract
    // (a bare request stays bare).
    let request = request_with_model("gpt-5.5", None);
    assert_eq!(super::ChatGptBackendClient::instructions(&request), "");
}

/// The tool-call batching contract rides at the tail of the composed
/// instructions for every model on this backend: base-prompt prose alone left
/// GPT models at ~2.6 tool calls per tool-using message vs ~3.5 for Claude on
/// the same harness, and each unbatched call is a full extra round trip.
#[test]
fn instructions_append_tool_batching_contract() {
    let mut request = request_with_model("gpt-5.6-sol", None);
    request.system = Some(crate::types::system_from_string("be terse"));
    let instructions = super::ChatGptBackendClient::instructions(&request);
    assert!(
        instructions.ends_with(super::TOOL_BATCHING_CONTRACT),
        "batching contract must close the instructions: {instructions}"
    );
    // The contract must not displace the identity override from the head.
    assert!(
        instructions.starts_with("You are gpt-5.6"),
        "identity stays first: {instructions}"
    );
}

#[test]
fn parses_non_stream_output_to_message_response() {
    let value = json!({
        "id":"resp_9",
        "output":[
            {"type":"message","content":[{"type":"output_text","text":"hi"}]},
            {"type":"function_call","call_id":"c1","name":"read","arguments":"{\"p\":1}"}
        ],
        "usage":{"input_tokens":4,"output_tokens":2,
            "input_tokens_details":{"cached_tokens":3}}
    });
    let response = super::parse_responses_response(&value, "gpt-5.5", "");
    assert_eq!(response.id, "resp_9");
    assert_eq!(response.content.len(), 2);
    assert!(matches!(
        &response.content[0],
        OutputContentBlock::Text { text } if text == "hi"
    ));
    match &response.content[1] {
        OutputContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id, "c1");
            assert_eq!(name, "read");
            assert_eq!(input, &json!({ "p": 1 }));
        }
        other => panic!("expected tool use, got {other:?}"),
    }
    assert_eq!(response.usage.input_tokens, 1);
    assert_eq!(response.usage.cache_read_input_tokens, 3);
    assert_eq!(response.usage.output_tokens, 2);
}

/// The true non-streaming `send_message` path (`parse_responses_response`)
/// assembles `MessageResponse.reasoning_replay` from the same `output` array
/// walk the streaming completed-response path uses, and records the entry in
/// the session-scoped cache fallback too.
#[test]
fn parses_non_stream_output_populates_reasoning_replay_and_caches_it() {
    let value = json!({
        "id":"resp_10",
        "output":[
            {"type":"reasoning","id":"rs_ns","encrypted_content":"OPAQUE-NS"},
            {"type":"function_call","call_id":"c_ns","name":"read","arguments":"{}"}
        ],
        "usage":{"input_tokens":1,"output_tokens":1}
    });
    let response = super::parse_responses_response(&value, "gpt-5.5", "session-ns");
    let replay = response
        .reasoning_replay
        .expect("non-streaming response must carry the assembled reasoning replay");
    assert_eq!(
        replay,
        json!([{"call_id": "c_ns", "items": [
            {"type": "reasoning", "id": "rs_ns", "encrypted_content": "OPAQUE-NS"}
        ]}])
    );

    // The same entry lands in the session-scoped cache fallback.
    let cached = reasoning_for_call("session-ns", "c_ns").expect("cached under its session");
    assert_eq!(cached[0]["encrypted_content"], json!("OPAQUE-NS"));
}

#[test]
fn stream_idle_timeout_defaults_and_env_override() {
    let _guard = env_lock();
    let key = super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV;
    let restore = std::env::var(key).ok();

    std::env::remove_var(key);
    assert_eq!(
        super::stream_idle_timeout(),
        Some(std::time::Duration::from_millis(
            super::CHATGPT_STREAM_IDLE_TIMEOUT_MS
        )),
        "default budget applies when unset"
    );

    std::env::set_var(key, "1500");
    assert_eq!(
        super::stream_idle_timeout(),
        Some(std::time::Duration::from_millis(1_500)),
        "valid override is honoured"
    );

    std::env::set_var(key, "0");
    assert_eq!(
        super::stream_idle_timeout(),
        None,
        "zero disables the idle timeout"
    );

    std::env::set_var(key, "not-a-number");
    assert_eq!(
        super::stream_idle_timeout(),
        Some(std::time::Duration::from_millis(
            super::CHATGPT_STREAM_IDLE_TIMEOUT_MS
        )),
        "garbage falls back to the default"
    );

    let startup_key = super::CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_ENV;
    let startup_restore = std::env::var(startup_key).ok();
    std::env::remove_var(startup_key);
    assert_eq!(
        super::startup_no_progress_timeout(),
        Some(std::time::Duration::from_millis(
            super::CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_MS
        ))
    );
    std::env::set_var(startup_key, "250");
    assert_eq!(
        super::startup_no_progress_timeout(),
        Some(std::time::Duration::from_millis(250))
    );
    std::env::set_var(startup_key, "0");
    assert_eq!(super::startup_no_progress_timeout(), None);

    match restore {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
    match startup_restore {
        Some(value) => std::env::set_var(startup_key, value),
        None => std::env::remove_var(startup_key),
    }
}

#[test]
fn startup_reasoning_extends_deadline_exactly_once() {
    let window = std::time::Duration::from_secs(240);
    let initial = std::time::Instant::now()
        .checked_add(window)
        .expect("initial deadline");
    let mut deadline = Some(initial);
    let mut extended = false;

    super::extend_startup_deadline_for_reasoning(
        &mut deadline,
        Some(window),
        &mut extended,
    );
    let once = deadline.expect("extended deadline");
    assert_eq!(once, initial.checked_add(window).unwrap());
    assert!(extended);

    super::extend_startup_deadline_for_reasoning(
        &mut deadline,
        Some(window),
        &mut extended,
    );
    assert_eq!(deadline, Some(once), "later reasoning must not extend again");
}

#[test]
fn terminal_failure_recovery_request_disables_stream_and_deescalates_xhigh() {
    let request = MessageRequest {
        model: "gpt-5.5".into(),
        messages: vec![InputMessage::user_text("hi")],
        effort: Some(EffortLevel::Xhigh),
        thinking: Some(ThinkingConfig::enabled(16_000)),
        stream: true,
        ..request(vec![], None, None)
    };

    let recovered = super::deescalated_recovery_request(&request);

    assert!(!recovered.stream);
    assert_eq!(recovered.effort, Some(EffortLevel::High));
    assert!(recovered.thinking.is_none());
    assert!(recovered.output_config.is_none());
}

#[test]
fn terminal_failure_recovery_request_preserves_auto_effort() {
    let request = MessageRequest {
        model: "gpt-5.5".into(),
        messages: vec![InputMessage::user_text("hi")],
        effort: None,
        stream: true,
        ..request(vec![], None, None)
    };

    let recovered = super::deescalated_recovery_request(&request);

    assert!(!recovered.stream);
    assert_eq!(recovered.effort, None);
    assert!(recovered.thinking.is_none());
    assert!(recovered.output_config.is_none());
}

#[test]
fn restart_is_armed_only_before_commit_and_within_budget() {
    // Pre-commit, retryable, budget available → restart.
    assert!(should_restart(false, true, 0, 5));
    assert!(should_restart(false, true, 4, 5));

    // Committed (output already surfaced) → never restart, even on a
    // retryable fault with budget left. This is the duplicate-output guard.
    assert!(!should_restart(true, true, 0, 5));

    // Non-retryable fault (e.g. auth) → propagate regardless of arming.
    assert!(!should_restart(false, false, 0, 5));

    // Budget exhausted → stop. `attempts == max_retries` is already spent.
    assert!(!should_restart(false, true, 5, 5));
    assert!(!should_restart(false, true, 6, 5));

    // Zero budget → no transparent restart at all (idle timeout still
    // surfaces as a retryable error for an outer caller to handle).
    assert!(!should_restart(false, true, 0, 0));
}

#[test]
fn idle_timeout_error_drives_a_restart_decision() {
    // The exact error the stalled-stream path raises must be classified as
    // restart-eligible while the turn is still re-armable.
    let err = ApiError::stream_idle_timeout(std::time::Duration::from_secs(90));
    assert!(should_restart(false, err.is_retryable(), 0, 5));
    assert!(!should_restart(true, err.is_retryable(), 0, 5));
}

#[test]
fn restart_commit_boundary_ignores_reasoning_prefix() {
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    let reasoning_prefix = [
        json!({"type":"response.created","response":{"id":"r1"}}),
        json!({
            "type":"response.output_item.added",
            "output_index":0,
            "item":{"type":"reasoning"}
        }),
        json!({
            "type":"response.reasoning_summary_text.delta",
            "output_index":0,
            "delta":"Assessing implementation quality"
        }),
    ];

    for value in reasoning_prefix {
        for event in state.ingest(&value) {
            assert!(
                !crosses_restart_commit_boundary(&event),
                "reasoning/bookkeeping event should remain replay-safe: {event:?}"
            );
        }
    }

    let text_events = state.ingest(&json!({
        "type":"response.output_text.delta",
        "output_index":1,
        "delta":"visible"
    }));
    assert!(
        text_events.iter().any(crosses_restart_commit_boundary),
        "visible answer text must lock out transparent restart"
    );
}

#[test]
fn backoff_grows_then_caps() {
    let client = super::ChatGptBackendClient::new("token", None).with_retry_policy(
        5,
        std::time::Duration::from_millis(500),
        std::time::Duration::from_secs(4),
    );
    // 500ms · 2^(n-1): 500, 1000, 2000, then capped at 4s. The method now
    // delegates to the shared `providers::backoff_for_attempt`, which returns a
    // `Result` (Err on shift overflow) — unified with the OpenAI/Anthropic path.
    assert_eq!(
        client.backoff_for_attempt(1).unwrap(),
        std::time::Duration::from_millis(500)
    );
    assert_eq!(
        client.backoff_for_attempt(2).unwrap(),
        std::time::Duration::from_secs(1)
    );
    assert_eq!(
        client.backoff_for_attempt(3).unwrap(),
        std::time::Duration::from_secs(2)
    );
    assert_eq!(
        client.backoff_for_attempt(4).unwrap(),
        std::time::Duration::from_secs(4)
    );
    // A far-out attempt overflows the doubling shift and surfaces a
    // BackoffOverflow error rather than saturating; production caps retries far
    // below this, so the `?` at the call site never trips it in practice.
    assert!(client.backoff_for_attempt(40).is_err());
}

/// A transport that sends only SSE comments is alive at the socket layer but
/// has made no model progress. Those keep-alives must not postpone the startup
/// deadline; the uncommitted request is safe to restart exactly once here.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn response_header_stall_is_bounded_before_stream_construction() {
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut scratch = [0u8; 1024];
        let _ = socket.read(&mut scratch).await;
        // Accept the request but never send an HTTP status line or headers.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    });

    let idle_key = super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV;
    let startup_key = super::CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_ENV;
    let restore_idle = std::env::var(idle_key).ok();
    let restore_startup = std::env::var(startup_key).ok();
    std::env::set_var(idle_key, "100");
    std::env::set_var(startup_key, "1000");

    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"));
    let started = std::time::Instant::now();
    let error = client
        .stream_message(&request(vec![InputMessage::user_text("hi")], None, None))
        .await
        .expect_err("an unanswered HTTP open must time out");

    match restore_idle {
        Some(value) => std::env::set_var(idle_key, value),
        None => std::env::remove_var(idle_key),
    }
    match restore_startup {
        Some(value) => std::env::set_var(startup_key, value),
        None => std::env::remove_var(startup_key),
    }
    server.abort();

    assert!(
        error.to_string().contains("stream_idle_timeout"),
        "the shorter byte-idle budget should classify the header stall: {error}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_millis(800),
        "header timeout must fire without waiting for the server task"
    );
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn keepalive_only_stream_restarts_at_startup_progress_deadline() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 1024];
        let _ = first.read(&mut scratch).await;
        first
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        first.flush().await.unwrap();
        let keepalive_writer = tokio::spawn(async move {
            for _ in 0..40 {
                if first.write_all(b": keepalive\n\n").await.is_err() {
                    break;
                }
                if first.flush().await.is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });

        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        let body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r2\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
            "\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,",
            "\"delta\":\"recovered after keepalive stall\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":",
            "{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        let _ = keepalive_writer.await;
    });

    let startup_key = super::CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_ENV;
    let restore = std::env::var(startup_key).ok();
    std::env::set_var(startup_key, "150");
    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(
            1,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(20),
        );
    let mut stream = client
        .stream_message(&request(vec![InputMessage::user_text("hi")], None, None))
        .await
        .expect("open stream");

    let mut text = String::new();
    while let Some(event) = stream.next_event().await.expect("restart should recover") {
        if let StreamEvent::ContentBlockDelta(delta) = event {
            if let ContentBlockDelta::TextDelta { text: chunk } = delta.delta {
                text.push_str(&chunk);
            }
        }
    }
    match restore {
        Some(value) => std::env::set_var(startup_key, value),
        None => std::env::remove_var(startup_key),
    }
    server.await.unwrap();

    assert_eq!(text, "recovered after keepalive stall");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// Reasoning is useful startup activity, but it is not a task action. The first
/// delta grants one extension; a backend that then streams reasoning forever
/// must still be restarted when the extended deadline expires. This exercises
/// the real `emitted == true` path rather than the keepalive-only branch above.
// end-to-end reasoning-restart test; body exceeds the 100-line lint threshold
#[allow(clippy::too_many_lines)]
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn continuous_reasoning_restarts_after_single_startup_extension() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 1024];
        let _ = first.read(&mut scratch).await;
        first
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        first
            .write_all(concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
                "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
                "\"item\":{\"type\":\"reasoning\"}}\n\n",
            ).as_bytes())
            .await
            .unwrap();
        first.flush().await.unwrap();
        tokio::spawn(async move {
            for _ in 0..100 {
                if first
                    .write_all(concat!(
                        "data: {\"type\":\"response.reasoning_summary_text.delta\",",
                        "\"output_index\":0,\"delta\":\"still thinking\"}\n\n",
                    ).as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
                if first.flush().await.is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });

        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        let body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r2\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
            "\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,",
            "\"delta\":\"recovered after bounded reasoning\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":",
            "{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
    });

    let startup_key = super::CHATGPT_STARTUP_NO_PROGRESS_TIMEOUT_ENV;
    let restore_startup = std::env::var(startup_key).ok();
    std::env::set_var(startup_key, "100");
    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(
            1,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(20),
        );
    let mut stream = client
        .stream_message(&request(vec![InputMessage::user_text("hi")], None, None))
        .await
        .expect("open stream");

    let outcome = tokio::time::timeout(std::time::Duration::from_millis(900), async {
        let mut text = String::new();
        while let Some(event) = stream.next_event().await? {
            if let StreamEvent::ContentBlockDelta(delta) = event {
                if let ContentBlockDelta::TextDelta { text: chunk } = delta.delta {
                    text.push_str(&chunk);
                }
            }
        }
        Ok::<_, ApiError>(text)
    })
    .await;
    match restore_startup {
        Some(value) => std::env::set_var(startup_key, value),
        None => std::env::remove_var(startup_key),
    }
    if outcome.is_err() {
        server.abort();
    }
    let text = outcome
        .expect("continuous reasoning must not bypass the extended startup deadline")
        .expect("restart should recover");
    server.await.unwrap();

    assert_eq!(text, "recovered after bounded reasoning");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// End-to-end proof that a pre-commit stall recovers over a real socket:
/// the mock server's first connection sends only headers and then hangs
/// (the silent-reasoning case), and the second connection serves a full SSE
/// turn. With a sub-second idle budget the stream must idle out, restart,
/// and yield the recovered text — exactly once, with no error.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn stalled_precommit_stream_restarts_and_recovers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        // Connection 1: send headers, then go silent forever (until the
        // client gives up and drops us). This is the stalled stream.
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 1024];
        let _ = first.read(&mut scratch).await;
        first
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        first.flush().await.unwrap();
        // Hold the first connection open without blocking the listener from
        // accepting the client's restart (real HTTP servers accept both
        // concurrently).
        let first_holder = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            drop(first);
        });

        // Connection 2 (the restart): serve a complete SSE turn.
        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        let body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
            "\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,",
            "\"delta\":\"recovered\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":",
            "{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        first_holder.abort();
    });

    // Sub-second idle budget so the stalled connection trips quickly.
    std::env::set_var(super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV, "300");
    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(
            3,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );

    let request = MessageRequest {
        messages: vec![InputMessage::user_text("hi")],
        ..request(vec![], None, None)
    };
    let mut stream = client.stream_message(&request).await.expect("open stream");

    let mut text = String::new();
    let mut events = 0;
    while let Some(event) = stream.next_event().await.expect("no error after restart") {
        events += 1;
        if let StreamEvent::ContentBlockDelta(delta) = &event {
            if let ContentBlockDelta::TextDelta { text: chunk } = &delta.delta {
                text.push_str(chunk);
            }
        }
    }
    std::env::remove_var(super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV);
    server.await.unwrap();

    assert_eq!(
        text, "recovered",
        "recovered turn must stream after restart"
    );
    assert!(events > 0, "stream should yield events");
    // The server was hit exactly twice: the stalled attempt + the restart.
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// A response body that terminates before its declared length makes reqwest fail
/// while reading `Response::chunk`, not while opening the response. It must be
/// retried before a text or tool-argument commit exactly like a stalled stream.
#[tokio::test]
async fn truncated_precommit_response_body_restarts_and_recovers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 4096];
        let _ = first.read(&mut scratch).await;
        // The content length deliberately exceeds the actual SSE payload. A
        // peer close after this write therefore becomes reqwest's body-decode
        // error on the next `Response::chunk()` call.
        let incomplete = "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n";
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            incomplete.len() + 64
        );
        first.write_all(head.as_bytes()).await.unwrap();
        first.write_all(incomplete.as_bytes()).await.unwrap();
        first.flush().await.unwrap();
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        let body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r2\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
            "\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,",
            "\"delta\":\"recovered after truncated body\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":",
            "{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
    });

    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(
            1,
            std::time::Duration::from_millis(1),
            std::time::Duration::from_millis(1),
        );
    let mut stream = client
        .stream_message(&request(vec![InputMessage::user_text("hi")], None, None))
        .await
        .expect("open initial stream");

    let mut text = String::new();
    while let Some(event) = stream
        .next_event()
        .await
        .expect("truncated pre-commit body should restart")
    {
        if let StreamEvent::ContentBlockDelta(delta) = event {
            if let ContentBlockDelta::TextDelta { text: chunk } = delta.delta {
                text.push_str(&chunk);
            }
        }
    }
    server.await.unwrap();

    assert_eq!(text, "recovered after truncated body");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn restart_budget_exhaustion_is_structural() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        let mut holders = Vec::new();
        for _ in 0..2 {
            let (mut conn, _) = listener.accept().await.unwrap();
            server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut scratch = [0u8; 1024];
            let _ = conn.read(&mut scratch).await;
            conn.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
                .await
                .unwrap();
            conn.flush().await.unwrap();
            holders.push(tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                drop(conn);
            }));
        }
        for holder in holders {
            holder.await.unwrap();
        }
    });

    std::env::set_var(super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV, "300");
    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(
            1,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );
    let request = MessageRequest {
        messages: vec![InputMessage::user_text("hi")],
        ..request(vec![], None, None)
    };
    let mut stream = client.stream_message(&request).await.expect("open stream");

    let error = stream.next_event().await.expect_err("budget exhausted");
    std::env::remove_var(super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV);
    assert!(
        matches!(
            error,
            ApiError::RetriesExhausted { attempts: 2, last_error }
                if matches!(last_error.as_ref(), ApiError::StreamApi { error_type, .. }
                    if error_type.as_deref() == Some("stream_idle_timeout"))
        ),
        "expected exhausted wrapper around the final idle-timeout"
    );
    server.await.unwrap();
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// The real terminal frame a Codex refusal that names no reason takes: per the
/// Responses schema `ResponseError` carries `code` AND `message` together, or is
/// `null`, so `"error": null` is the only legal shape for "refused, no reason".
const UNATTRIBUTED_FAILURE_FRAME: &str = concat!(
    "data: {\"type\":\"response.failed\",",
    "\"response\":{\"id\":\"resp_x\",\"status\":\"failed\",\"error\":null}}\n\n",
);

const SHED_RECOVERY_FRAMES: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"recovered after shed\"}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r_shed\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n",
);

/// Read one whole HTTP request (headers + `content-length` body) so a test can
/// assert on what was actually sent — a single `read` would truncate the
/// multi-megabyte image body these tests are about.
async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;

    let mut raw = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    let mut header_end = None;
    while header_end.is_none() {
        let read = socket.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        header_end = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4);
    }
    if let Some(start) = header_end {
        let head = String::from_utf8_lossy(&raw[..start]).to_ascii_lowercase();
        let length = head
            .split("content-length:")
            .nth(1)
            .and_then(|rest| rest.split("\r\n").next())
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while raw.len() - start < length {
            let read = socket.read(&mut chunk).await.unwrap_or(0);
            if read == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..read]);
        }
    }
    String::from_utf8_lossy(&raw).into_owned()
}

/// Serve one scripted SSE body per successive connection, recording every
/// request body. `None` drops the connection unanswered — the shape a refused
/// request takes, and what makes the de-escalated recovery fail.
async fn scripted_responses_backend(
    listener: tokio::net::TcpListener,
    script: Vec<Option<&'static str>>,
    bodies: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    use tokio::io::AsyncWriteExt;

    for reply in script {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let request = read_http_request(&mut socket).await;
        bodies.lock().expect("bodies lock").push(request);
        let Some(body) = reply else {
            drop(socket);
            continue;
        };
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(body.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        drop(socket);
    }
}

/// Drive one turn against a scripted backend.
///
/// Returns the events the caller actually received, the error that ended the
/// turn (if any), and the raw request bodies the backend saw. Events and error
/// are returned SEPARATELY on purpose: a turn that surfaces text and then fails
/// must be distinguishable from one that swallows the text, and a `Result`
/// return would discard exactly that evidence.
async fn drive_scripted_turn(
    messages: Vec<InputMessage>,
    script: Vec<Option<&'static str>>,
) -> (Vec<StreamEvent>, Option<ApiError>, Vec<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = tokio::spawn(scripted_responses_backend(
        listener,
        script,
        bodies.clone(),
    ));

    // No restarts: the connection sequence is then exactly the initial stream,
    // the de-escalated one-shot recovery, and any shed retry.
    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(
            0,
            std::time::Duration::from_millis(1),
            std::time::Duration::from_millis(1),
        );
    let request = MessageRequest {
        messages,
        ..request(vec![], None, None)
    };
    let mut stream = client.stream_message(&request).await.expect("open stream");

    let mut events = Vec::new();
    let failure = loop {
        match stream.next_event().await {
            Ok(Some(event)) => events.push(event),
            Ok(None) => break None,
            Err(error) => break Some(error),
        }
    };
    server.abort();
    let captured = bodies.lock().expect("bodies lock").clone();
    (events, failure, captured)
}

/// Whether the turn surfaced `text` as a streamed text delta.
fn surfaced_text(events: &[StreamEvent], text: &str) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                delta: ContentBlockDelta::TextDelta { text: delta },
                ..
            }) if delta == text
        )
    })
}

/// A tool result carrying `bytes` of staged screenshot, the shape a browser/MCP
/// screenshot tool produces and every later request then replays.
fn screenshot_tool_result(bytes: usize) -> InputMessage {
    InputMessage {
        role: "user".to_string(),
        content: vec![InputContentBlock::ToolResult {
            tool_use_id: "call_screenshot".to_string(),
            content: vec![
                crate::types::ToolResultContentBlock::Text {
                    text: "screenshot captured".to_string(),
                },
                crate::types::ToolResultContentBlock::Image {
                    source: ImageSource {
                        kind: "base64".to_string(),
                        media_type: "image/png".to_string(),
                        data: format!("SHEDMARKER{}", "A".repeat(bytes)),
                    },
                },
            ],
            is_error: false,
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: None,
    }
}

/// Reproduction of the reported browser/MCP screenshot failure, end to end: the
/// backend refuses with `"error": null`, the de-escalated recovery re-sends the
/// same bytes and is refused too, and the ladder is spent.
///
/// Every attempt so far carried the identical payload, so the turn may not die
/// there: the accumulated screenshot is shed once and the retry — the first
/// request whose bytes actually differ — completes. The shed body must keep the
/// `[image image/png]` placeholder (the model still learns an image was there)
/// while carrying none of the pixels.
#[tokio::test]
async fn image_heavy_refusal_sheds_the_screenshot_and_recovers() {
    let (events, failure, bodies) = drive_scripted_turn(
        vec![screenshot_tool_result(1_200_000)],
        vec![
            Some(UNATTRIBUTED_FAILURE_FRAME),
            None,
            Some(SHED_RECOVERY_FRAMES),
        ],
    )
    .await;

    assert!(failure.is_none(), "the shed retry must recover the turn");
    assert!(
        surfaced_text(&events, "recovered after shed"),
        "recovered text was not surfaced: {events:?}"
    );

    assert_eq!(bodies.len(), 3, "initial stream, recovery, then the shed retry");
    assert!(
        bodies[0].contains("SHEDMARKER"),
        "the first request must carry the screenshot"
    );
    assert!(
        !bodies[2].contains("SHEDMARKER"),
        "the shed retry must carry none of the image bytes"
    );
    assert!(
        bodies[2].contains("[image image/png]"),
        "the shed retry must keep the placeholder: {}",
        &bodies[2][..bodies[2].len().min(2000)]
    );
}

/// The shape a real browser/MCP session has: every screenshot is stored beside
/// a much larger TEXT artifact (accessibility snapshot, DOM dump, network
/// listing), so the session that motivated this carries more text than pixels.
///
/// Requiring the images to be an outright majority would decline exactly this
/// case and let the turn die, which is why the gate asks for a third instead.
#[tokio::test]
async fn screenshot_beside_bigger_snapshot_text_is_still_shed() {
    let snapshot_text = InputMessage {
        role: "user".to_string(),
        content: vec![InputContentBlock::ToolResult {
            tool_use_id: "call_snapshot".to_string(),
            content: vec![crate::types::ToolResultContentBlock::Text {
                text: "a11y-node ".repeat(200_000),
            }],
            is_error: false,
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: None,
    };

    // Images are the minority (1.2 MB against 2 MB of snapshot text) but still a
    // leading term, so shedding is the move that can plausibly rescue the turn.
    let (events, failure, bodies) = drive_scripted_turn(
        vec![screenshot_tool_result(1_200_000), snapshot_text],
        vec![
            Some(UNATTRIBUTED_FAILURE_FRAME),
            None,
            Some(SHED_RECOVERY_FRAMES),
        ],
    )
    .await;

    assert!(
        failure.is_none(),
        "a text-heavy browser session must still get its shed retry: {failure:?}"
    );
    assert!(surfaced_text(&events, "recovered after shed"), "{events:?}");
    assert_eq!(bodies.len(), 3, "the shed retry must be spent");
    assert!(
        !bodies[2].contains("SHEDMARKER"),
        "the shed retry must carry none of the image bytes"
    );
    assert!(
        bodies[2].contains("a11y-node"),
        "only the pixels are shed; the text artifact stays"
    );
}

/// One small image is not evidence of a size problem. The identical wire
/// failure must spend no extra request and surface exactly as before —
/// `RetriesExhausted` wrapping a `Transient` fault, so the ordinary retry /
/// model-fallback path is untouched.
#[tokio::test]
async fn small_image_refusal_is_not_shed() {
    let (_events, failure, bodies) = drive_scripted_turn(
        vec![screenshot_tool_result(64)],
        vec![Some(UNATTRIBUTED_FAILURE_FRAME), None, Some(SHED_RECOVERY_FRAMES)],
    )
    .await;

    let error = failure.expect("a small-image refusal must still surface");
    assert!(matches!(error, ApiError::RetriesExhausted { .. }), "{error}");
    assert_eq!(
        error.provider_error_class(),
        crate::error::ProviderErrorClass::Transient,
        "classification must be untouched: {error}"
    );
    assert_eq!(bodies.len(), 2, "no shed retry may be spent: {bodies:?}");
}

/// A text-only turn has nothing to shed, so the refusal surfaces unchanged.
#[tokio::test]
async fn text_only_refusal_is_not_shed() {
    let (_events, failure, bodies) = drive_scripted_turn(
        vec![InputMessage::user_text("hi")],
        vec![Some(UNATTRIBUTED_FAILURE_FRAME), None, Some(SHED_RECOVERY_FRAMES)],
    )
    .await;

    let error = failure.expect("a text-only refusal must still surface");
    assert!(matches!(error, ApiError::RetriesExhausted { .. }), "{error}");
    assert_eq!(
        error.provider_error_class(),
        crate::error::ProviderErrorClass::Transient,
        "{error}"
    );
    assert_eq!(bodies.len(), 2, "no shed retry may be spent: {bodies:?}");
}

/// One transport chunk can carry visible text AND the terminal failure frame.
///
/// Three things must all hold for that chunk. The text the backend already
/// produced must reach the caller — it is valid output, and swallowing it is a
/// silent loss. It must be delivered exactly once, never re-sent behind a
/// recovered response. And because it crossed the commit boundary on its way
/// out, the turn may not be replayed afterwards: no shed, no restart, no
/// non-streaming recovery, so the backend is contacted exactly once.
#[tokio::test]
async fn same_chunk_text_and_failure_surfaces_text_then_the_error() {
    const COMMITTED_THEN_FAILED: &str = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"already streamed\"}\n\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":null}}\n\n",
    );

    let (events, failure, bodies) = drive_scripted_turn(
        vec![screenshot_tool_result(1_200_000)],
        vec![Some(COMMITTED_THEN_FAILED), Some(SHED_RECOVERY_FRAMES)],
    )
    .await;

    assert!(
        surfaced_text(&events, "already streamed"),
        "text decoded before the failure must still reach the caller: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                    delta: ContentBlockDelta::TextDelta { .. },
                    ..
                })
            ))
            .count(),
        1,
        "and exactly once — a recovered response must not repeat it: {events:?}"
    );

    let error = failure.expect("a committed failure must still propagate");
    assert!(
        matches!(error, ApiError::StreamApi { .. }),
        "a committed failure is not wrapped as exhausted: {error}"
    );
    assert_eq!(bodies.len(), 1, "a committed turn must not re-open: {bodies:?}");
}

/// When the shed retry is refused too, the turn surfaces the original exhausted
/// error and stops — the one-shot flag must not let a second shed run.
#[tokio::test]
async fn shed_retry_failure_surfaces_the_original_error_once() {
    let (_events, failure, bodies) = drive_scripted_turn(
        vec![screenshot_tool_result(1_200_000)],
        vec![
            Some(UNATTRIBUTED_FAILURE_FRAME),
            None,
            Some(UNATTRIBUTED_FAILURE_FRAME),
            None,
            Some(SHED_RECOVERY_FRAMES),
        ],
    )
    .await;

    let error = failure.expect("a twice-refused turn must surface");
    assert!(matches!(error, ApiError::RetriesExhausted { .. }), "{error}");
    assert_eq!(
        bodies.len(),
        4,
        "exactly one shed retry (plus its own recovery attempt): {}",
        bodies.len()
    );
}

#[tokio::test]
async fn terminal_failure_before_commit_falls_back_to_non_stream_response() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 4096];
        let _ = first.read(&mut scratch).await;
        let first_body = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{}}}\n\n";
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            first_body.len()
        );
        first.write_all(head.as_bytes()).await.unwrap();
        first.write_all(first_body.as_bytes()).await.unwrap();
        first.flush().await.unwrap();
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        // The one-shot recovery request is itself a STREAMED call: the Codex
        // Responses endpoint rejects `stream: false` outright (400 `Stream must
        // be set to true`), so `send_message` issues a stream and folds the SSE
        // body back into one response. The recovered text therefore arrives as
        // deltas, with the terminal frame carrying an empty `output` exactly
        // like the live backend sends it.
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"recovered nonstream\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r2\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
    });

    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(
            0,
            std::time::Duration::from_millis(1),
            std::time::Duration::from_millis(1),
        );
    let request = MessageRequest {
        model: "gpt-5.5".into(),
        messages: vec![InputMessage::user_text("hi")],
        effort: Some(EffortLevel::Xhigh),
        ..request(vec![], None, None)
    };
    let mut stream = client.stream_message(&request).await.expect("open stream");

    let mut recovered = false;
    while let Some(event) = stream.next_event().await.expect("fallback should recover") {
        if let StreamEvent::MessageStart(start) = event {
            recovered = start.message.content.iter().any(|block| {
                matches!(
                    block,
                    OutputContentBlock::Text { text } if text == "recovered nonstream"
                )
            });
        }
    }
    server.await.unwrap();

    assert!(recovered, "non-stream fallback response was not surfaced");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// Same socket-level proof as the empty pre-commit stall, but with the
/// real gpt-5.5 shape that triggered the TUI freeze: Responses emits
/// message/reasoning frames, then goes silent before any visible answer text
/// or tool arguments. Those frames are safe to replay, so the stream must
/// still restart instead of wedging the turn until the user interrupts.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn stalled_stream_after_reasoning_prefix_restarts_and_recovers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 1024];
        let _ = first.read(&mut scratch).await;
        let first_body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
            "\"item\":{\"type\":\"reasoning\"}}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",",
            "\"output_index\":0,\"delta\":\"Assessing implementation quality\"}\n\n",
        );
        first
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        first.write_all(first_body.as_bytes()).await.unwrap();
        first.flush().await.unwrap();
        let first_holder = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            drop(first);
        });

        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        let second_body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r2\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
            "\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,",
            "\"delta\":\"recovered\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":",
            "{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            second_body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(second_body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        first_holder.abort();
    });

    std::env::set_var(super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV, "300");
    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(
            3,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );

    let request = MessageRequest {
        messages: vec![InputMessage::user_text("hi")],
        ..request(vec![], None, None)
    };
    let mut stream = client.stream_message(&request).await.expect("open stream");

    let mut text = String::new();
    let mut reasoning_chunks = 0;
    while let Some(event) = stream.next_event().await.expect("restart should recover") {
        if let StreamEvent::ContentBlockDelta(delta) = &event {
            match &delta.delta {
                ContentBlockDelta::TextDelta { text: chunk } => text.push_str(chunk),
                ContentBlockDelta::ThinkingDelta { .. } => reasoning_chunks += 1,
                _ => {}
            }
        }
    }
    std::env::remove_var(super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV);
    server.await.unwrap();

    assert_eq!(reasoning_chunks, 1, "first attempt surfaced reasoning");
    assert_eq!(
        text, "recovered",
        "second attempt should stream the recovered answer"
    );
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

// The mid-stream restart notice callback fires when a pre-commit stall
// transparently re-opens the upstream connection — the otherwise-silent pause
// a live UI needs to show as "reconnecting" instead of a freeze.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn mid_stream_restart_invokes_retry_notice_callback() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // First attempt: emit only a reasoning prefix (no commit), then go
        // silent so the per-chunk idle timeout fires and forces a restart.
        let (mut first, _) = listener.accept().await.unwrap();
        let mut scratch = [0u8; 1024];
        let _ = first.read(&mut scratch).await;
        let first_body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
            "\"item\":{\"type\":\"reasoning\"}}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",",
            "\"output_index\":0,\"delta\":\"thinking\"}\n\n",
        );
        first
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        first.write_all(first_body.as_bytes()).await.unwrap();
        first.flush().await.unwrap();
        let first_holder = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            drop(first);
        });

        // Second attempt: a clean, complete turn.
        let (mut second, _) = listener.accept().await.unwrap();
        let _ = second.read(&mut scratch).await;
        let second_body = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r2\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
            "\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,",
            "\"delta\":\"ok\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":",
            "{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            second_body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(second_body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        first_holder.abort();
    });

    std::env::set_var(super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV, "300");
    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_retry_policy(
            3,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );

    let request = MessageRequest {
        messages: vec![InputMessage::user_text("hi")],
        ..request(vec![], None, None)
    };

    let notices = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
    let sink = notices.clone();
    let mut stream = client
        .stream_message(&request)
        .await
        .expect("open stream")
        .with_retry_notice_callback(move |notice| {
            sink.lock()
                .unwrap()
                .push((notice.attempt, notice.max_attempts));
        });

    while let Some(_event) = stream.next_event().await.expect("restart should recover") {}
    std::env::remove_var(super::CHATGPT_STREAM_IDLE_TIMEOUT_ENV);
    server.await.unwrap();

    let seen = notices.lock().unwrap().clone();
    assert!(
        !seen.is_empty(),
        "a mid-stream transparent restart must fire the retry-notice callback"
    );
    assert_eq!(seen[0], (1, 3), "first restart is attempt 1 of max_retries 3");
}

#[tokio::test]
async fn default_retry_budget_recovers_on_eighth_attempt() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();
    let server = tokio::spawn(async move {
        for attempt in 1..=8 {
            let (mut connection, _) = listener.accept().await.unwrap();
            server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut request = [0u8; 4096];
            let _ = connection.read(&mut request).await;
            let body = if attempt < 8 {
                "data: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"retry\"}\n\n"
            } else {
                concat!(
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r8\"}}\n\n",
                    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
                    "\"item\":{\"type\":\"message\"}}\n\n",
                    "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,",
                    "\"delta\":\"recovered\"}\n\n",
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0}\n\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"usage\":",
                    "{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
                )
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
                body.len()
            );
            connection.write_all(head.as_bytes()).await.unwrap();
            connection.write_all(body.as_bytes()).await.unwrap();
            connection.flush().await.unwrap();
        }
    });

    let mut client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"));
    client.initial_backoff = std::time::Duration::from_millis(1);
    client.max_backoff = std::time::Duration::from_millis(2);
    let request = MessageRequest {
        messages: vec![InputMessage::user_text("hi")],
        ..request(vec![], None, None)
    };
    let notices = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = notices.clone();
    let mut stream = client
        .stream_message(&request)
        .await
        .expect("initial stream opens")
        .with_retry_notice_callback(move |notice| {
            sink.lock()
                .unwrap()
                .push((notice.attempt, notice.max_attempts));
        });

    let mut text = String::new();
    while let Some(event) = stream.next_event().await.expect("eighth attempt recovers") {
        if let StreamEvent::ContentBlockDelta(delta) = event {
            if let ContentBlockDelta::TextDelta { text: chunk } = delta.delta {
                text.push_str(&chunk);
            }
        }
    }
    server.await.unwrap();

    assert_eq!(text, "recovered");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 8);
    assert_eq!(
        notices.lock().unwrap().as_slice(),
        [(1, 7), (2, 7), (3, 7), (4, 7), (5, 7), (6, 7), (7, 7)]
    );
}

// A large frame split across many small chunks parses exactly once and the
// separator search stays linear (the GPT `encrypted_content` shape that
// used to pin a core and freeze the TUI). See `ResponsesSseParser::scanned`.
#[test]
fn large_frame_split_across_many_chunks_parses_once() {
    let big = "x".repeat(512 * 1024);
    let frame = format!(
        "data: {{\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"{big}\"}}"
    );
    let bytes = frame.as_bytes();
    let mut parser = ResponsesSseParser::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let end = (offset + 1024).min(bytes.len());
        assert!(parser
            .push(&bytes[offset..end])
            .expect("normal large frame stays under the cap")
            .is_empty());
        offset = end;
    }
    let events = parser.push(b"\n\n").expect("terminator completes the frame");
    assert_eq!(
        events.len(),
        1,
        "frame must parse exactly once after terminator"
    );
}

// A stream that never emits a frame separator must be rejected once the retained
// buffer would exceed the crate-wide SSE cap, rather than growing without bound.
#[test]
fn oversized_unterminated_frame_is_rejected() {
    let mut parser = ResponsesSseParser::new();
    let chunk = vec![b'x'; crate::sse::MAX_SSE_BUFFER_BYTES + 1];
    let error = parser
        .push(&chunk)
        .expect_err("a chunk past the cap must be rejected");
    assert!(
        matches!(error, ApiError::InvalidSseFrame(_)),
        "expected invalid sse frame, got {error:?}"
    );
}

// A `\r\n\r\n` separator split byte-by-byte across chunk boundaries (so it
// straddles the scan resume point) must still be detected.
#[test]
fn separator_split_across_chunks_is_found() {
    let mut parser = ResponsesSseParser::new();
    assert!(parser
        .push(
            b"data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}"
        )
        .expect("partial frame buffers")
        .is_empty());
    assert!(parser.push(b"\r").expect("partial separator buffers").is_empty());
    assert!(parser.push(b"\n").expect("partial separator buffers").is_empty());
    assert!(parser.push(b"\r").expect("partial separator buffers").is_empty());
    let events = parser.push(b"\n").expect("completed separator parses");
    assert_eq!(events.len(), 1, "split CRLFCRLF separator must be detected");
}

/// `response.incomplete` (the model spent its whole output budget, usually
/// on reasoning) must close the message with an honest `max_tokens` stop —
/// ignoring it ended the stream with zero events, which the runtime
/// misread as "no assistant content" and retried the identical request
/// forever (the 2026-06-11 empty-response loop).
#[test]
fn incomplete_close_emits_max_tokens_stop_instead_of_silence() {
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    assert!(
        !state
            .ingest(&json!({"type":"response.created","response":{"id":"resp_1"}}))
            .is_empty()
    );
    let events = state.ingest(&json!({
        "type": "response.incomplete",
        "response": {
            "incomplete_details": { "reason": "max_output_tokens" },
            "usage": { "input_tokens": 10, "output_tokens": 2048 },
        },
    }));
    let stop_reason = events.iter().find_map(|event| match event {
        StreamEvent::MessageDelta(delta) => delta.delta.stop_reason.clone(),
        _ => None,
    });
    assert_eq!(stop_reason.as_deref(), Some("max_tokens"));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageStop(_))),
        "incomplete must terminate the message"
    );
    // Terminal: later frames are ignored exactly like after `completed`.
    assert!(
        state
            .ingest(&json!({"type":"response.output_text.delta","output_index":0,"delta":"x"}))
            .is_empty()
    );
}

/// `response.failed` / top-level `error` frames surface as a real stream
/// error (retryable for server faults), never as a silent empty stream.
#[test]
fn failed_event_surfaces_as_stream_error_not_empty_stream() {
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    let events = state.ingest(&json!({
        "type": "response.failed",
        "response": { "error": { "code": "server_error", "message": "boom" } },
    }));
    assert!(events.is_empty(), "failure is not a display event");
    let failure = state.take_failure().expect("failure must be recorded");
    assert!(failure.is_retryable(), "server faults are retryable");
    assert!(state.take_failure().is_none(), "failure drains once");

    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    state.ingest(&json!({
        "type": "error",
        "code": "invalid_request_error",
        "message": "bad input item",
    }));
    let failure = state.take_failure().expect("error frame must be recorded");
    assert!(
        !failure.is_retryable(),
        "invalid-request class is not retryable"
    );
}

/// The SSE twin of the websocket rule: a `usage_limit_reached` failure frame is
/// this account's 429 with the reset it named, not a retryable stream fault.
#[test]
fn a_usage_limit_failure_frame_is_this_accounts_429_with_its_reset() {
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    state.ingest(&json!({
        "type": "error",
        "code": "usage_limit_reached",
        "message": "The usage limit has been reached",
        "resets_in_seconds": 8220,
    }));
    let failure = state.take_failure().expect("error frame must be recorded");
    assert!(!failure.is_retryable(), "a plan window is not a stream fault");
    assert_eq!(
        failure.provider_error_class(),
        crate::ProviderErrorClass::account_rate_limit(Some(std::time::Duration::from_secs(8220)))
    );
    let text = failure.to_string();
    assert!(text.contains("429") && text.contains("retry-after: 8220"), "text: {text}");
}

/// A terminal frame naming neither a code nor a message must still carry enough
/// detail to tell a refused request from a transient fault — while keeping the
/// substring the retry classifier and the pre-commit recovery both key on.
///
/// The frame is provider-controlled, so the digest is an allowlisted projection
/// of scalar diagnostic fields: no member outside that list — known content
/// (prompts, model output), nested error payloads, or a field this build has
/// never seen — may reach an error string that is displayed, logged, and
/// persisted to the session transcript.
#[test]
fn message_less_failure_frame_carries_bounded_digest() {
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    state.ingest(&json!({
        "type": "response.failed",
        "unknown_top_level": "UNKNOWN TOP SECRET",
        // An allowlisted pointer holding a non-scalar must be skipped whole.
        "param": {"nested": "OBJECT PARAM SECRET"},
        "response": {
            "id": "resp_1",
            "status": "failed",
            "error": {"details": ["NESTED DETAIL SECRET"]},
            "instructions": "SYSTEM PROMPT TEXT",
            "output": [{"type": "message", "content": "MODEL OUTPUT TEXT"}],
            "unknown_member": "UNKNOWN RESPONSE SECRET",
        },
    }));
    let failure = state.take_failure().expect("failure must be recorded");
    let text = failure.to_string();
    assert!(
        text.contains("backend reported a terminal stream failure"),
        "{text}"
    );
    assert!(text.contains("type=\"response.failed\""), "{text}");
    assert!(text.contains("status=\"failed\""), "{text}");
    for secret in [
        "SYSTEM PROMPT TEXT",
        "MODEL OUTPUT TEXT",
        "NESTED DETAIL SECRET",
        "UNKNOWN TOP SECRET",
        "UNKNOWN RESPONSE SECRET",
        "OBJECT PARAM SECRET",
    ] {
        assert!(!text.contains(secret), "{secret} leaked into {text}");
    }
    assert!(failure.is_retryable(), "an uncoded fault stays retryable");
    assert!(super::is_terminal_stream_failure(&failure), "{text}");

    // A long allowlisted value is truncated rather than pasted in whole.
    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    state.ingest(&json!({"type": "error", "param": "p".repeat(500)}));
    let failure = state.take_failure().expect("failure must be recorded");
    let text = failure.to_string();
    assert!(text.contains('…'), "{text}");
    assert!(text.len() < 300, "digest must stay bounded: {text}");

    let mut state = ResponsesStreamState::new("gpt-5.5".to_string());
    state.ingest(&json!({"type": "error"}));
    let failure = state.take_failure().expect("failure must be recorded");
    assert_eq!(
        failure.to_string(),
        "api stream error: backend reported a terminal stream failure (type=\"error\")"
    );
}

/// An empty-response retry/continuation reminder in the system blocks must
/// step the reasoning effort down — replaying the identical xhigh request
/// deterministically reproduces the empty turn.
#[test]
fn empty_retry_reminder_de_escalates_reasoning_effort() {
    // Contract pin: the runtime's reminder prefixes
    // (crates/runtime/src/conversation/mod.rs) — if these literals drift,
    // the de-escalation silently stops firing.
    assert_eq!(
        super::EMPTY_RETRY_REMINDER_MARKER,
        "[zo:empty-response-retry]"
    );
    assert_eq!(
        super::EMPTY_CONTINUATION_REMINDER_MARKER,
        "[zo:empty-response-continuation]"
    );

    let reminder_system = Some(vec![
        crate::types::SystemBlock::Text {
            text: "base operating manual".to_string(),
            cache_control: None,
        },
        crate::types::SystemBlock::Text {
            text: "[zo:empty-response-retry] <system-reminder>retry now</system-reminder>"
                .to_string(),
            cache_control: None,
        },
    ]);

    // Legacy GPT projects a 24k Max budget to xhigh, so retry pressure
    // de-escalates one step to medium.
    let mut req = request_with_model("gpt-5.5", Some(24_000));
    req.system.clone_from(&reminder_system);
    let body = build_responses_request(&req, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("medium"));

    // Same request without the reminder keeps the requested tier.
    let clean = build_responses_request(&request_with_model("gpt-5.5", Some(24_000)), "i", true);
    assert_eq!(clean["reasoning"]["effort"], json!("xhigh"));

    // User-selected `/effort xhigh` / `ultracode` is an explicit top-effort
    // contract: retry pressure must not silently lower it, including on GPT fast.
    let mut explicit = request_with_model("gpt-5.5-fast", None);
    explicit.effort = Some(crate::types::EffortLevel::Xhigh);
    explicit.system.clone_from(&reminder_system);
    let body = build_responses_request(&explicit, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"));
    assert_eq!(body["service_tier"], json!("priority"));

    let mut explicit_max = request_with_model("gpt-5.5-fast", None);
    explicit_max.effort = Some(crate::types::EffortLevel::Max);
    explicit_max.system.clone_from(&reminder_system);
    let body = build_responses_request(&explicit_max, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("xhigh"));
    assert_eq!(body["service_tier"], json!("priority"));

    // Lower explicit efforts keep the existing empty-response retry behavior:
    // they still step down instead of being protected by the top-tier exception.
    let mut explicit_high = request_with_model("gpt-5.5-fast", None);
    explicit_high.effort = Some(crate::types::EffortLevel::High);
    explicit_high.system.clone_from(&reminder_system);
    let body = build_responses_request(&explicit_high, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("medium"));
    assert_eq!(body["service_tier"], json!("priority"));

    let mut explicit_medium = request_with_model("gpt-5.5-fast", None);
    explicit_medium.effort = Some(crate::types::EffortLevel::Medium);
    explicit_medium.system.clone_from(&reminder_system);
    let body = build_responses_request(&explicit_medium, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("low"));
    assert_eq!(body["service_tier"], json!("priority"));

    // The continuation reminder (post-fallback turns) de-escalates budget-derived effort too.
    let mut cont = request_with_model("gpt-5.5", Some(24_000));
    cont.system = Some(vec![crate::types::SystemBlock::Text {
        text: "[zo:empty-response-continuation] <system-reminder>state intact</system-reminder>"
            .to_string(),
        cache_control: None,
    }]);
    let body = build_responses_request(&cont, "i", true);
    assert_eq!(body["reasoning"]["effort"], json!("medium"));
}


#[test]
fn sse_fold_rebuilds_output_from_text_deltas() {
    // Codex runs with `store: false`: the terminal frame arrives with
    // `"output": []` and the answer exists only in the delta events. Returning
    // that frame as-is produced a well-formed response with NO content, which
    // is what made every `send_message` caller (the routing probe among them)
    // read a perfectly good reply as malformed.
    let body = concat!(
        "data: {\"type\":\"response.created\"}\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"complexity\\\":\"}\n",
        "data: not json\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"\\\"medium\\\"}\"}\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"output\":[]}}\n",
        "data: [DONE]\n",
    );

    let folded = completed_response_from_sse(body).expect("terminal frame present");
    assert_eq!(folded["id"], json!("r1"));
    assert_eq!(
        folded["output"][0]["content"][0]["text"],
        json!("{\"complexity\":\"medium\"}"),
        "the streamed deltas must be folded back into `output`"
    );
}

#[test]
fn sse_fold_keeps_a_terminal_frame_that_already_carries_output() {
    // Only fill the gap — never overwrite a real payload.
    let body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ignored\"}\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"text\":\"real\"}]}]}}\n",
    );

    let folded = completed_response_from_sse(body).expect("terminal frame present");
    assert_eq!(folded["output"][0]["content"][0]["text"], json!("real"));
}

#[test]
fn sse_fold_accepts_an_incomplete_terminal_frame() {
    // A turn cut short by the token cap is still an answer worth returning.
    let body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n",
        "data: {\"type\":\"response.incomplete\",\"response\":{\"output\":[]}}\n",
    );

    let folded = completed_response_from_sse(body).expect("incomplete is terminal too");
    assert_eq!(folded["output"][0]["content"][0]["text"], json!("partial"));
}

#[test]
fn sse_fold_returns_none_without_a_terminal_frame() {
    let body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"orphan\"}\n";
    assert!(completed_response_from_sse(body).is_none());
}

// ---------------------------------------------------------------------------
// Turn-boundary reasoning replay
//
// The per-call replay above keeps reasoning continuity across a tool call. The
// tests below cover the other side of the turn boundary: reasoning that
// preceded the turn's FINAL TEXT (a plain answer, a plan, an attempt that
// stopped to explain). Before this existed, any turn ending in text discarded
// its reasoning permanently, so every later turn re-derived it from scratch —
// the exact behaviour OpenAI measured as a 3x agentic-benchmark regression.
// ---------------------------------------------------------------------------

/// Mirror of what the runtime does between two turns: fold a provider response
/// into the assistant history entry that the next request re-encodes
/// (`response_to_events` -> `build_assistant_message` -> `convert_messages`).
/// Block order and the carried `reasoning_replay` payload match that path.
fn assistant_history_entry(response: &crate::types::MessageResponse) -> InputMessage {
    InputMessage {
        role: "assistant".into(),
        content: response
            .content
            .iter()
            .map(|block| match block {
                OutputContentBlock::Text { text } => InputContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                },
                OutputContentBlock::ToolUse { id, name, input } => InputContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    cache_control: None,
                },
                OutputContentBlock::Thinking { .. } | OutputContentBlock::RedactedThinking { .. } => {
                    unreachable!("the Responses backend never emits Anthropic thinking blocks")
                }
            })
            .collect(),
        thought_signature: None,
        reasoning_replay: response.reasoning_replay.clone(),
    }
}

/// Round-trip a payload through the exact representation the session file uses:
/// `ConversationMessage::to_json` stores it as embedded JSON *text* and the
/// loader parses it back with `serde_json` (see
/// `core_types::ConversationMessage::reasoning_replay`).
fn through_session_json(payload: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&payload.to_string()).expect("session-persisted payload re-parses")
}

/// A response that ends in text: its reasoning items must be captured, and
/// replayed in the NEXT request immediately before the assistant message —
/// the position they held in the original response.
#[test]
fn turn_final_reasoning_replays_before_the_final_assistant_message() {
    let session = "session-turn-final-text";
    let response = parse_responses_response(
        &json!({
            "id": "resp_1",
            "output": [
                {"type": "reasoning", "id": "rs_final", "encrypted_content": "OPAQUE-FINAL"},
                {"type": "message", "role": "assistant",
                 "content": [{"type": "output_text", "text": "Here is the plan."}]}
            ]
        }),
        "gpt-5.6-sol",
        session,
    );
    assert_eq!(
        response.reasoning_replay,
        Some(json!([
            {"turn_final": true, "items": [
                {"type": "reasoning", "id": "rs_final", "encrypted_content": "OPAQUE-FINAL"}
            ]}
        ])),
        "a text-ending turn must capture its reasoning"
    );

    let body = build_responses_request_for_session(
        &request(
            vec![
                InputMessage::user_text("plan it"),
                assistant_history_entry(&response),
                InputMessage::user_text("now do it"),
            ],
            None,
            None,
        ),
        "i",
        true,
        session,
    );
    let input = body["input"].as_array().expect("input array");
    assert_eq!(input[0]["type"], json!("message"), "user turn");
    assert_eq!(input[1]["type"], json!("reasoning"));
    assert_eq!(input[1]["id"], json!("rs_final"));
    assert_eq!(input[1]["encrypted_content"], json!("OPAQUE-FINAL"));
    assert_eq!(
        input[2]["type"],
        json!("message"),
        "the reasoning item must be immediately followed by the message it preceded"
    );
    assert_eq!(input[2]["role"], json!("assistant"));
    assert_eq!(input[2]["content"][0]["text"], json!("Here is the plan."));
    assert_eq!(input[3]["role"], json!("user"));
    assert_eq!(input.len(), 4, "no stray items: {input:?}");
}

/// Tool-call-only path: capture payload and wire encoding are byte-identical to
/// the pre-change behaviour — no turn-final entry exists to add.
#[test]
fn tool_call_only_turn_encodes_byte_identically() {
    let session = "session-turn-final-toolonly";
    let output = vec![
        json!({"type": "reasoning", "id": "rs_a", "encrypted_content": "OPAQUE-A"}),
        json!({"type": "function_call", "call_id": "call_a", "name": "read",
               "arguments": "{\"path\":\"a.rs\"}"}),
    ];
    let payload = reasoning_replay_from_output(&output, session);
    assert_eq!(
        payload,
        Some(json!([
            {"call_id": "call_a", "items": [
                {"type": "reasoning", "id": "rs_a", "encrypted_content": "OPAQUE-A"}
            ]}
        ])),
        "a tool-call-only response must produce exactly the legacy payload"
    );

    let assistant = InputMessage {
        role: "assistant".into(),
        content: vec![InputContentBlock::ToolUse {
            id: "call_a".into(),
            name: "read".into(),
            input: json!({"path": "a.rs"}),
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: payload,
    };
    let body = build_responses_request_for_session(
        &request(vec![InputMessage::user_text("go"), assistant], None, None),
        "i",
        true,
        session,
    );
    let input = body["input"].as_array().expect("input array");
    // Compared as `Value`, not as a serialized string. Object key order is not
    // part of the wire contract — it is decided by whether `serde_json`'s
    // `preserve_order` feature is on, and that is set by *feature unification*,
    // not by this crate: `agent-client-protocol` enables it, so every build that
    // links `acp` (which includes the shipped `zo` binary and any
    // `cargo test --workspace` run) gets insertion-ordered maps, while
    // `cargo test -p api` alone gets `BTreeMap`'s alphabetical order. A
    // hard-coded string therefore pinned the one ordering production never uses
    // and made this test pass or fail based on which crates were in the build
    // graph. `Value` equality ignores key order while still pinning exactly what
    // this test is about: the item sequence, and that no item gained, lost, or
    // shifted a field.
    assert_eq!(
        serde_json::Value::Array(input.clone()),
        json!([
            {"type": "message", "role": "user",
             "content": [{"type": "input_text", "text": "go"}]},
            {"type": "reasoning", "id": "rs_a", "encrypted_content": "OPAQUE-A"},
            {"type": "function_call", "call_id": "call_a", "name": "read",
             "arguments": "{\"path\":\"a.rs\"}"},
        ]),
        "tool-only encoding must carry exactly the legacy items and fields"
    );

    remove_reasoning_for_call(session, "call_a");
}

/// Mixed turn — reasoning A, `function_call`, reasoning B, final text: A replays
/// before the call (unchanged), B replays before the final message, and neither
/// group is emitted twice.
#[test]
fn mixed_turn_replays_each_reasoning_group_exactly_once() {
    let session = "session-turn-final-mixed";
    let response = parse_responses_response(
        &json!({
            "id": "resp_mixed",
            "output": [
                {"type": "reasoning", "id": "rs_a", "encrypted_content": "OPAQUE-A"},
                {"type": "function_call", "call_id": "call_m", "name": "read",
                 "arguments": "{\"path\":\"m.rs\"}"},
                {"type": "reasoning", "id": "rs_b", "encrypted_content": "OPAQUE-B"},
                {"type": "message", "role": "assistant",
                 "content": [{"type": "output_text", "text": "Read it; here's why."}]}
            ]
        }),
        "gpt-5.6-sol",
        session,
    );
    assert_eq!(
        response.reasoning_replay,
        Some(json!([
            {"call_id": "call_m", "items": [
                {"type": "reasoning", "id": "rs_a", "encrypted_content": "OPAQUE-A"}
            ]},
            {"turn_final": true, "items": [
                {"type": "reasoning", "id": "rs_b", "encrypted_content": "OPAQUE-B"}
            ]}
        ])),
        "each reasoning group is attributed to exactly one anchor"
    );

    let body = build_responses_request_for_session(
        &request(
            vec![
                InputMessage::user_text("go"),
                assistant_history_entry(&response),
            ],
            None,
            None,
        ),
        "i",
        true,
        session,
    );
    let input = body["input"].as_array().expect("input array");
    // The replayed prefix reproduces the original response order exactly.
    assert_eq!(input[1]["id"], json!("rs_a"));
    assert_eq!(input[2]["type"], json!("function_call"));
    assert_eq!(input[2]["call_id"], json!("call_m"));
    assert_eq!(input[3]["id"], json!("rs_b"));
    assert_eq!(input[4]["type"], json!("message"));
    assert_eq!(input[4]["role"], json!("assistant"));
    assert_eq!(input.len(), 5, "nothing replayed twice: {input:?}");
    let reasoning_ids: Vec<&str> = input
        .iter()
        .filter(|item| item["type"] == json!("reasoning"))
        .map(|item| item["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(reasoning_ids, vec!["rs_a", "rs_b"]);

    remove_reasoning_for_call(session, "call_m");
}

/// The payload survives the session file's embedded-JSON-text representation:
/// a resumed session still replays the turn-final items.
#[test]
fn turn_final_replay_survives_the_session_json_round_trip() {
    let session = "session-turn-final-roundtrip";
    let payload = reasoning_replay_from_output(
        &[
            json!({"type": "reasoning", "id": "rs_rt", "encrypted_content": "OPAQUE-RT"}),
            json!({"type": "message", "role": "assistant",
                   "content": [{"type": "output_text", "text": "answer"}]}),
        ],
        session,
    )
    .expect("text-ending turn captures a payload");

    let assistant = InputMessage {
        role: "assistant".into(),
        content: vec![InputContentBlock::Text {
            text: "answer".into(),
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: Some(through_session_json(&payload)),
    };
    let body = build_responses_request_for_session(
        &request(vec![InputMessage::user_text("q"), assistant], None, None),
        "i",
        true,
        session,
    );
    let input = body["input"].as_array().expect("input array");
    assert_eq!(input[1]["type"], json!("reasoning"));
    assert_eq!(input[1]["id"], json!("rs_rt"));
    assert_eq!(input[1]["encrypted_content"], json!("OPAQUE-RT"));
    assert_eq!(input[2]["type"], json!("message"));
}

/// Old-schema payload (`call_id`-only entries, written by a build that predates
/// turn-final capture) still replays per-call, and the absent turn-final entry
/// is simply a no-op rather than an error.
#[test]
fn legacy_call_id_only_payload_still_replays_per_call() {
    let session = "session-turn-final-legacy";
    let legacy = through_session_json(&json!([
        {"call_id": "call_legacy", "items": [
            {"type": "reasoning", "id": "rs_legacy", "encrypted_content": "OPAQUE-LEGACY"}
        ]}
    ]));
    let assistant = InputMessage {
        role: "assistant".into(),
        content: vec![
            InputContentBlock::ToolUse {
                id: "call_legacy".into(),
                name: "read".into(),
                input: json!({}),
                cache_control: None,
            },
            InputContentBlock::Text {
                text: "and here is what I found".into(),
                cache_control: None,
            },
        ],
        thought_signature: None,
        reasoning_replay: Some(legacy),
    };
    let body = build_responses_request_for_session(
        &request(vec![InputMessage::user_text("go"), assistant], None, None),
        "i",
        true,
        session,
    );
    let input = body["input"].as_array().expect("input array");
    assert_eq!(input[1]["type"], json!("reasoning"));
    assert_eq!(input[1]["id"], json!("rs_legacy"));
    assert_eq!(input[2]["type"], json!("function_call"));
    assert_eq!(input[2]["call_id"], json!("call_legacy"));
    assert_eq!(input[3]["type"], json!("message"));
    assert_eq!(input[3]["role"], json!("assistant"));
    assert_eq!(
        input.len(),
        4,
        "no turn-final entry exists, so nothing extra is emitted: {input:?}"
    );
}

/// A response that ends in reasoning with nothing after it (e.g. `incomplete`
/// after spending the whole budget thinking) must capture nothing. Replaying a
/// reasoning item whose following item is absent is a hard 400 from the
/// Responses API (`Item 'rs_...' of type 'reasoning' was provided without its
/// required following item`), so capture — not just replay — has to refuse it.
#[test]
fn reasoning_with_no_following_item_is_never_captured() {
    assert_eq!(
        reasoning_replay_from_output(
            &[json!({"type": "reasoning", "id": "rs_dangling"})],
            "session-turn-final-dangling",
        ),
        None
    );
}

/// Reasoning before a *mid-turn* message (one followed by a tool call) is
/// dropped rather than misplaced: replay has a single turn-final slot, before
/// the message flushed at the end of the assistant turn, and this message is
/// flushed earlier. Dropping matches the pre-change behaviour; emitting it at
/// the wrong offset would not.
#[test]
fn reasoning_before_a_mid_turn_message_is_dropped_not_misplaced() {
    let session = "session-turn-final-midmessage";
    let payload = reasoning_replay_from_output(
        &[
            json!({"type": "reasoning", "id": "rs_mid"}),
            json!({"type": "message", "role": "assistant",
                   "content": [{"type": "output_text", "text": "checking"}]}),
            json!({"type": "reasoning", "id": "rs_call"}),
            json!({"type": "function_call", "call_id": "call_mid", "name": "read",
                   "arguments": "{}"}),
        ],
        session,
    );
    assert_eq!(
        payload,
        Some(json!([
            {"call_id": "call_mid", "items": [{"type": "reasoning", "id": "rs_call"}]}
        ])),
        "only the call-anchored group is captured"
    );
    remove_reasoning_for_call(session, "call_mid");
}

/// The streaming path captures the turn-final group too: it is rebuilt from the
/// terminal frame's authoritative `output` snapshot and rides the closing
/// `message_delta`, the same channel the per-call entries use.
#[test]
fn streaming_terminal_frame_carries_the_turn_final_group() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string())
        .with_session_id("session-turn-final-stream");
    let events = state.ingest(&json!({
        "type": "response.completed",
        "response": {
            "id": "resp_stream",
            "usage": {"input_tokens": 10, "output_tokens": 3},
            "output": [
                {"type": "reasoning", "id": "rs_stream", "encrypted_content": "OPAQUE-STREAM"},
                {"type": "message", "role": "assistant",
                 "content": [{"type": "output_text", "text": "done"}]}
            ]
        }
    }));
    let replay = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::MessageDelta(delta) => delta.delta.reasoning_replay.clone(),
            _ => None,
        })
        .expect("closing message_delta carries the payload");
    assert_eq!(
        replay,
        json!([
            {"turn_final": true, "items": [
                {"type": "reasoning", "id": "rs_stream", "encrypted_content": "OPAQUE-STREAM"}
            ]}
        ])
    );
}

/// Drive the state the way the LIVE backend does: reasoning and text arrive as
/// streaming frames and the terminal frame carries `"output": []`.
///
/// This is the shape every real Codex turn has (`store: false`, see
/// `build_responses_request_for_session`), and reading only the terminal frame
/// meant the replay fold ran over an empty array on every one of them — so the
/// attached payload was `None` always, the turn-final leg (which has no cache
/// fallback by design) never fired even once, and every text-ending turn threw
/// its reasoning away for the model to re-derive on the next turn. The tests
/// above could not catch it: they hand-build the populated `output` the live
/// backend never sends.
#[test]
fn a_live_shaped_turn_whose_terminal_frame_is_empty_still_carries_its_reasoning() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string())
        .with_session_id("session-empty-terminal-frame");
    state.ingest(&json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": {"type": "reasoning", "id": "rs_live", "encrypted_content": "OPAQUE-LIVE"}
    }));
    state.ingest(&json!({
        "type": "response.output_text.delta",
        "output_index": 1,
        "delta": "here is the plan"
    }));
    let events = state.ingest(&json!({
        "type": "response.completed",
        "response": {
            "id": "resp_live",
            "usage": {"input_tokens": 10, "output_tokens": 3},
            "output": []
        }
    }));

    let replay = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::MessageDelta(delta) => delta.delta.reasoning_replay.clone(),
            _ => None,
        })
        .expect("a turn that ended in text must carry its turn-final reasoning");
    assert_eq!(
        replay,
        json!([
            {"turn_final": true, "items": [
                {"type": "reasoning", "id": "rs_live", "encrypted_content": "OPAQUE-LIVE"}
            ]}
        ])
    );
}

/// Same live shape, but the turn ends in a tool call rather than text.
#[test]
fn a_live_shaped_tool_call_turn_carries_its_per_call_reasoning() {
    let session = "session-empty-terminal-frame-call";
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string()).with_session_id(session);
    state.ingest(&json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": {"type": "reasoning", "id": "rs_call", "encrypted_content": "OPAQUE-CALL"}
    }));
    state.ingest(&json!({
        "type": "response.output_item.done",
        "output_index": 1,
        "item": {"type": "function_call", "call_id": "call_live", "name": "Read",
                 "arguments": "{}"}
    }));
    let events = state.ingest(&json!({
        "type": "response.completed",
        "response": {"id": "resp_live_call", "usage": {"input_tokens": 4, "output_tokens": 1},
                     "output": []}
    }));

    let replay = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::MessageDelta(delta) => delta.delta.reasoning_replay.clone(),
            _ => None,
        })
        .expect("the attached payload is what survives a restart; the cache does not");
    assert_eq!(
        replay,
        json!([
            {"call_id": "call_live", "items": [
                {"type": "reasoning", "id": "rs_call", "encrypted_content": "OPAQUE-CALL"}
            ]}
        ])
    );
    remove_reasoning_for_call(session, "call_live");
}

/// The shape the LIVE backend actually sends: a terminal frame that is not
/// empty, but carries no `reasoning` items.
///
/// This is the case an emptiness test misses. Codex runs with `store: false`,
/// so it retains no reasoning to hand back — the frame lists the turn's
/// `message` and `function_call` items and nothing else. Folding a replay
/// payload out of that yields nothing, which is why the attached payload was
/// `None` on every real turn even though the reasoning items had arrived
/// moments earlier on `response.output_item.done`.
#[test]
fn a_terminal_frame_without_reasoning_falls_back_to_the_streamed_items() {
    let session = "session-frame-without-reasoning";
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string()).with_session_id(session);
    state.ingest(&json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": {"type": "reasoning", "id": "rs_streamed", "encrypted_content": "STREAMED"}
    }));
    state.ingest(&json!({
        "type": "response.output_item.done",
        "output_index": 1,
        "item": {"type": "function_call", "call_id": "call_live", "name": "Read", "arguments": "{}"}
    }));
    let events = state.ingest(&json!({
        "type": "response.completed",
        "response": {
            "id": "resp_no_reasoning",
            "usage": {"input_tokens": 9, "output_tokens": 2},
            // Populated — but reasoning-free, exactly as the live wire sends it.
            "output": [
                {"type": "function_call", "call_id": "call_live", "name": "Read", "arguments": "{}"}
            ]
        }
    }));

    let replay = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::MessageDelta(delta) => delta.delta.reasoning_replay.clone(),
            _ => None,
        })
        .expect("a populated frame with no reasoning must not suppress the fallback");
    assert_eq!(
        replay,
        json!([
            {"call_id": "call_live", "items": [
                {"type": "reasoning", "id": "rs_streamed", "encrypted_content": "STREAMED"}
            ]}
        ])
    );
    remove_reasoning_for_call(session, "call_live");
}

/// A turn that spent its whole budget thinking and produced NOTHING must not
/// claim a turn-final group.
///
/// Replaying a reasoning item whose following item is absent is a hard 400
/// (`Item 'rs_…' of type 'reasoning' was provided without its required
/// following item`), so the reconstruction may only assert a turn-final
/// position when a message really will occupy it. This is the failure mode a
/// naive "just use the streamed items" fallback would have introduced.
#[test]
fn a_reasoning_only_turn_claims_no_turn_final_position() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string())
        .with_session_id("session-reasoning-only");
    state.ingest(&json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": {"type": "reasoning", "id": "rs_only", "encrypted_content": "OPAQUE-ONLY"}
    }));
    let events = state.ingest(&json!({
        "type": "response.incomplete",
        "response": {
            "id": "resp_incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "usage": {"input_tokens": 10, "output_tokens": 4000},
            "output": []
        }
    }));

    let replay = events.iter().find_map(|event| match event {
        StreamEvent::MessageDelta(delta) => delta.delta.reasoning_replay.clone(),
        _ => None,
    });
    assert_eq!(
        replay, None,
        "with no message to precede, a replayed reasoning item is a 400"
    );
}

/// A terminal frame that DOES carry `output` wins outright — the
/// reconstruction fills a gap and never overwrites a real payload, the same
/// rule `completed_response_from_sse` follows for the answer text.
#[test]
fn a_populated_terminal_frame_is_authoritative_over_the_streamed_items() {
    let mut state = ResponsesStreamState::new("gpt-5.6-sol".to_string())
        .with_session_id("session-authoritative-frame");
    // A streamed frame the terminal snapshot then contradicts: only the
    // snapshot's item may appear in the payload.
    state.ingest(&json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": {"type": "reasoning", "id": "rs_streamed", "encrypted_content": "STREAMED"}
    }));
    let events = state.ingest(&json!({
        "type": "response.completed",
        "response": {
            "id": "resp_authoritative",
            "usage": {"input_tokens": 10, "output_tokens": 3},
            "output": [
                {"type": "reasoning", "id": "rs_snapshot", "encrypted_content": "SNAPSHOT"},
                {"type": "message", "role": "assistant",
                 "content": [{"type": "output_text", "text": "ok"}]}
            ]
        }
    }));

    let replay = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::MessageDelta(delta) => delta.delta.reasoning_replay.clone(),
            _ => None,
        })
        .expect("payload");
    assert_eq!(
        replay,
        json!([
            {"turn_final": true, "items": [
                {"type": "reasoning", "id": "rs_snapshot", "encrypted_content": "SNAPSHOT"}
            ]}
        ])
    );
}

#[test]
fn image_generation_request_uses_the_hosted_tool_contract() {
    let body = super::build_image_generation_request(
        "gpt-5.6-sol[fast]",
        "a blue circle",
        Some("1024x1024"),
        Some("low"),
    );

    assert_eq!(body["model"], "gpt-5.6-sol");
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert_eq!(body["service_tier"], "priority");
    assert_eq!(body["tools"][0]["type"], "image_generation");
    assert_eq!(body["tools"][0]["size"], "1024x1024");
    assert_eq!(body["tools"][0]["quality"], "low");
    assert_eq!(body["tool_choice"]["type"], "image_generation");
    assert_eq!(
        body["input"][0]["content"][0]["text"],
        "a blue circle"
    );
}

#[test]
fn image_generation_parser_keeps_the_last_image_only_after_completion() {
    let complete = concat!(
        "data: {\"type\":\"response.image_generation_call.partial_image\",",
        "\"partial_image_b64\":\"EARLY\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{",
        "\"type\":\"image_generation_call\",\"result\":\"FINAL\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n",
    );
    let incomplete = concat!(
        "data: {\"type\":\"response.image_generation_call.partial_image\",",
        "\"partial_image_b64\":\"PARTIAL\"}\n\n",
    );

    assert_eq!(
        super::image_generation_result_from_sse(complete).expect("completed image"),
        "FINAL"
    );
    let error = super::image_generation_result_from_sse(incomplete)
        .expect_err("an unterminated image stream must fail");
    assert!(error.to_string().contains("before response.completed"));
}

#[test]
fn image_generation_parser_surfaces_provider_failure_messages() {
    let failed = concat!(
        "data: {\"type\":\"response.failed\",\"response\":{\"error\":{",
        "\"code\":\"rate_limit_exceeded\",\"message\":\"image quota exhausted\"}}}\n\n",
    );

    let error = super::image_generation_result_from_sse(failed)
        .expect_err("provider failure must be preserved");
    assert!(error.to_string().contains("image quota exhausted"));
    assert!(error.is_retryable());
}

#[tokio::test]
async fn image_generation_round_trips_over_the_chatgpt_backend() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let response = concat!(
        "data: {\"type\":\"response.image_generation_call.partial_image\",",
        "\"partial_image_b64\":\"IMAGE_BYTES\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n",
    );
    let server = tokio::spawn(scripted_responses_backend(
        listener,
        vec![Some(response)],
        bodies.clone(),
    ));
    let client = super::ChatGptBackendClient::new("token", Some("account".to_string()))
        .with_base_url(format!("http://{addr}"));

    let generated = client
        .generate_image("gpt-5.6-sol", "a test image", None, Some("low"))
        .await
        .expect("image response");
    assert_eq!(generated, "IMAGE_BYTES");
    server.await.unwrap();

    let request = bodies.lock().expect("bodies lock").pop().expect("request");
    let payload = request.split_once("\r\n\r\n").expect("HTTP body").1;
    let payload: serde_json::Value = serde_json::from_str(payload).expect("JSON request");
    assert_eq!(payload["tools"][0]["type"], "image_generation");
    assert_eq!(payload["tools"][0]["quality"], "low");
}

/// The WebSocket transport carries the same events as the SSE body — one text
/// frame per event, pings answered as keepalives — and the stream ends at the
/// response's terminal event while the socket stays open for the next request.
/// The handshake sends the WebSocket beta and the bearer; the first frame is
/// the HTTP body as a `response.create`.
#[allow(clippy::await_holding_lock, clippy::result_large_err)]
#[tokio::test]
async fn websocket_transport_streams_the_same_events_and_ends_at_the_terminal_frame() {
    type Seen = std::sync::Arc<std::sync::Mutex<(Vec<(String, String)>, Option<serde_json::Value>)>>;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Seen = std::sync::Arc::default();
    let server_seen = seen.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let headers_seen = server_seen.clone();
        let mut ws = tokio_tungstenite::accept_hdr_async(
            socket,
            move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                  response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                let mut seen = headers_seen.lock().unwrap();
                for (name, value) in request.headers() {
                    seen.0.push((
                        name.as_str().to_string(),
                        value.to_str().unwrap_or_default().to_string(),
                    ));
                }
                Ok(response)
            },
        )
        .await
        .unwrap();
        let Some(Ok(Message::Text(text))) = ws.next().await else {
            panic!("the first frame is the request");
        };
        server_seen.lock().unwrap().1 = Some(serde_json::from_str(&text).unwrap());
        let frames = [
            json!({"type":"response.created","response":{"id":"resp_ws"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","role":"assistant"}}),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"Hi"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{}}),
            json!({"type":"response.completed","response":{"usage":{"input_tokens":3,"output_tokens":4}}}),
        ];
        ws.send(Message::Text(frames[0].to_string())).await.unwrap();
        ws.send(Message::Ping(vec![1])).await.unwrap();
        for frame in &frames[1..] {
            ws.send(Message::Text(frame.to_string())).await.unwrap();
        }
        // The socket stays open: the client must end at the terminal event.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });

    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_transport(super::websocket::Transport::Websocket);
    let mut stream = client
        .stream_message(&request(vec![InputMessage::user_text("hi")], None, None))
        .await
        .expect("open over websocket");

    let started = std::time::Instant::now();
    let mut text = String::new();
    while let Some(event) = stream.next_event().await.expect("events") {
        if let StreamEvent::ContentBlockDelta(event) = event {
            if let ContentBlockDelta::TextDelta { text: delta } = event.delta {
                text.push_str(&delta);
            }
        }
    }
    server.abort();
    assert_eq!(text, "Hi");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the stream ends at the terminal event, not at the socket's close"
    );
    let (headers, create) = std::mem::take(&mut *seen.lock().unwrap());
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        header("openai-beta").as_deref(),
        Some(super::websocket::OPENAI_BETA_RESPONSES_WEBSOCKETS)
    );
    assert_eq!(header("authorization").as_deref(), Some("Bearer token"));
    assert_eq!(header("originator").as_deref(), Some(super::ORIGINATOR));
    let create = create.expect("the create frame was read");
    assert_eq!(create["type"], json!("response.create"));
    assert_eq!(create["model"], json!("gpt-5.6-sol"));
    assert_eq!(create["stream"], json!(true));
    assert!(create["input"].is_array(), "the HTTP body rides the frame: {create}");
}

/// Under `auto` only an https backend takes the socket, and only until a
/// handshake has failed in this process; `ws` forces it, `sse` refuses it.
#[test]
fn the_websocket_is_wanted_for_https_under_auto_and_follows_the_knob() {
    use super::websocket::Transport;
    let _guard = env_lock();
    let https = super::ChatGptBackendClient::new("token", None)
        .with_base_url("https://chatgpt.com/backend-api/codex/responses")
        .with_transport(Transport::Auto);
    let http = super::ChatGptBackendClient::new("token", None)
        .with_base_url("http://127.0.0.1:1/responses")
        .with_transport(Transport::Auto);
    super::WEBSOCKET_FALLBACK.store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(https.websocket_wanted(), "auto + https");
    assert!(!http.websocket_wanted(), "auto + http stays on SSE (mocks, proxies)");
    super::WEBSOCKET_FALLBACK.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(!https.websocket_wanted(), "one failed handshake turns the session to SSE");
    super::WEBSOCKET_FALLBACK.store(false, std::sync::atomic::Ordering::Relaxed);
    let forced = super::ChatGptBackendClient::new("token", None)
        .with_base_url("http://127.0.0.1:1/responses")
        .with_transport(Transport::Websocket);
    assert!(forced.websocket_wanted(), "forced");
    let refused = super::ChatGptBackendClient::new("token", None)
        .with_base_url("https://chatgpt.com/backend-api/codex/responses")
        .with_transport(Transport::Sse);
    assert!(!refused.websocket_wanted(), "refused");
}

/// A forced socket whose handshake fails is the error it met — no silent
/// fallback hides a misconfigured transport from the person who forced it.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_forced_websocket_whose_handshake_fails_is_an_error_not_a_fallback() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut scratch = [0u8; 2048];
        let _ = socket.read(&mut scratch).await;
        let _ = socket
            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
            .await;
    });
    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_transport(super::websocket::Transport::Websocket);
    let result = client
        .stream_message(&request(vec![InputMessage::user_text("hi")], None, None))
        .await;
    server.abort();
    let error = result.expect_err("a refused handshake is the error");
    assert!(error.to_string().contains("websocket"), "{error}");
}

fn tool_round_messages() -> (InputMessage, InputMessage) {
    let assistant = InputMessage {
        role: "assistant".into(),
        content: vec![InputContentBlock::ToolUse {
            id: "call_1".into(),
            name: "read".into(),
            input: json!({ "path": "x" }),
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: None,
    };
    let result = InputMessage {
        role: "user".into(),
        content: vec![InputContentBlock::ToolResult {
            tool_use_id: "call_1".into(),
            content: vec![crate::types::ToolResultContentBlock::Text {
                text: "data".into(),
            }],
            is_error: false,
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: None,
    };
    (assistant, result)
}

fn text_response_frames(id: &str, text: &str) -> Vec<serde_json::Value> {
    vec![
        json!({"type":"response.created","response":{"id":id}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","role":"assistant"}}),
        json!({"type":"response.output_text.delta","output_index":0,"delta":text}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg","role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}}),
        json!({"type":"response.completed","response":{"id":id,"usage":{"input_tokens":3,"output_tokens":4}}}),
    ]
}

async fn drain_text(stream: &mut super::ChatGptStream) -> String {
    let mut text = String::new();
    while let Some(event) = stream.next_event().await.expect("events") {
        if let StreamEvent::ContentBlockDelta(event) = event {
            if let ContentBlockDelta::TextDelta { text: delta } = event.delta {
                text.push_str(&delta);
            }
        }
    }
    text
}

/// A turn's second request rides the connection the first response left
/// behind: one socket, and the second `response.create` names the previous
/// response and carries only what is new after it — the tool result — not the
/// conversation again. codex's incremental requests.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn the_next_request_continues_the_previous_response_on_the_held_connection() {
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let frames: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
        std::sync::Arc::default();
    let (server_connections, server_frames) = (connections.clone(), frames.clone());
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            server_connections.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let frames = server_frames.clone();
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
                while let Some(Ok(Message::Text(text))) = ws.next().await {
                    let create: serde_json::Value = serde_json::from_str(&text).unwrap();
                    let nth = {
                        let mut frames = frames.lock().unwrap();
                        frames.push(create);
                        frames.len()
                    };
                    let response = if nth == 1 {
                        vec![
                            json!({"type":"response.created","response":{"id":"resp_1"}}),
                            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":""}}),
                            json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\": \"x\"}"}),
                            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{\"path\": \"x\"}","status":"completed"}}),
                            json!({"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":3,"output_tokens":4}}}),
                        ]
                    } else {
                        text_response_frames("resp_2", "Done")
                    };
                    for frame in response {
                        ws.send(Message::Text(frame.to_string())).await.unwrap();
                    }
                }
            });
        }
    });

    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_transport(super::websocket::Transport::Websocket);
    let first = request(vec![InputMessage::user_text("read x")], None, None);
    let mut stream = client.stream_message(&first).await.expect("first request");
    drain_text(&mut stream).await;

    let (assistant, result) = tool_round_messages();
    let second = request(
        vec![InputMessage::user_text("read x"), assistant, result],
        None,
        None,
    );
    let mut stream = client.stream_message(&second).await.expect("second request");
    let text = drain_text(&mut stream).await;
    server.abort();

    assert_eq!(text, "Done");
    assert_eq!(
        connections.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one connection carried both requests"
    );
    let frames = std::mem::take(&mut *frames.lock().unwrap());
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert!(
        frames[0].get("previous_response_id").is_none(),
        "the first request is whole: {}",
        frames[0]
    );
    assert_eq!(frames[1]["type"], json!("response.create"));
    assert_eq!(frames[1]["previous_response_id"], json!("resp_1"));
    assert_eq!(frames[1]["model"], frames[0]["model"]);
    let delta = frames[1]["input"].as_array().expect("input rides the frame");
    assert_eq!(delta.len(), 1, "only the tool result is new: {}", frames[1]["input"]);
    assert_eq!(delta[0]["type"], json!("function_call_output"));
    assert_eq!(delta[0]["call_id"], json!("call_1"));
}

/// A continuation the server no longer holds (`previous_response_not_found`)
/// goes out again whole on a fresh connection at once — no retry counted, no
/// backoff — as codex retries the full request.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_continuation_the_server_no_longer_holds_is_sent_whole_on_a_fresh_connection() {
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frames: std::sync::Arc<std::sync::Mutex<Vec<(usize, serde_json::Value)>>> =
        std::sync::Arc::default();
    let server_frames = frames.clone();
    let server = tokio::spawn(async move {
        let mut connection = 0;
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            connection += 1;
            let frames = server_frames.clone();
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
                while let Some(Ok(Message::Text(text))) = ws.next().await {
                    let create: serde_json::Value = serde_json::from_str(&text).unwrap();
                    let continuation = create.get("previous_response_id").is_some();
                    frames.lock().unwrap().push((connection, create));
                    let response = if continuation {
                        vec![json!({"type":"error","error":{"code":"previous_response_not_found","message":"Previous response was not found."}})]
                    } else if connection == 1 {
                        text_response_frames("resp_1", "Hi")
                    } else {
                        text_response_frames("resp_3", "Done")
                    };
                    for frame in response {
                        ws.send(Message::Text(frame.to_string())).await.unwrap();
                    }
                }
            });
        }
    });

    let client = super::ChatGptBackendClient::new("token", None)
        .with_base_url(format!("http://{addr}"))
        .with_transport(super::websocket::Transport::Websocket);
    let first = request(vec![InputMessage::user_text("hi")], None, None);
    let mut stream = client.stream_message(&first).await.expect("first request");
    assert_eq!(drain_text(&mut stream).await, "Hi");

    let answer = InputMessage {
        role: "assistant".into(),
        content: vec![InputContentBlock::Text {
            text: "Hi".into(),
            cache_control: None,
        }],
        thought_signature: None,
        reasoning_replay: None,
    };
    let second = request(
        vec![
            InputMessage::user_text("hi"),
            answer,
            InputMessage::user_text("and then?"),
        ],
        None,
        None,
    );
    let notices = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = notices.clone();
    let started = std::time::Instant::now();
    let mut stream = client
        .stream_message(&second)
        .await
        .expect("second request")
        .with_retry_notice_callback(move |_notice| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
    let text = drain_text(&mut stream).await;
    server.abort();

    assert_eq!(text, "Done");
    assert_eq!(notices.load(std::sync::atomic::Ordering::SeqCst), 0, "no retry was announced");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "no backoff");
    let frames = std::mem::take(&mut *frames.lock().unwrap());
    let shape: Vec<(usize, bool, usize)> = frames
        .iter()
        .map(|(connection, create)| {
            (
                *connection,
                create.get("previous_response_id").is_some(),
                create["input"].as_array().map_or(0, Vec::len),
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![(1, false, 1), (1, true, 1), (2, false, 3)],
        "whole, then a continuation the server refused, then whole again on a new connection: {frames:?}"
    );
}

