use super::{
    ChatCompletionChunk, DEFAULT_OPENAI_BASE_URL, OPENAI_REASONING_BLOCK_INDEX,
    OpenAiCompatClient, OpenAiCompatConfig, OpenAiSseParser, StreamState,
    build_chat_completion_request, chat_completions_endpoint, next_sse_frame,
    normalize_finish_reason, openai_tool_choice, parse_sse_frame, parse_tool_arguments,
    reasoning_effort_for,
};
use crate::error::ApiError;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    ImageSource, InputContentBlock, InputMessage, MessageRequest, OutputContentBlock, StreamEvent,
    ToolChoice, ToolDefinition, ToolResultContentBlock,
};
use serde_json::json;
use std::sync::{Mutex, OnceLock};

#[test]
fn request_translation_uses_openai_compatible_shape() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "grok-3".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage {
                role: "user".to_string(),
                content: vec![
                    InputContentBlock::Text {
                        text: "hello".to_string(),
                        cache_control: None,
                    },
                    InputContentBlock::ToolResult {
                        tool_use_id: "tool_1".to_string(),
                        content: vec![ToolResultContentBlock::Json {
                            value: json!({"ok": true}),
                        }],
                        is_error: false,
                                            cache_control: None,
                    },
                ],
                thought_signature: None,
                reasoning_replay: None,
            }],
            system: Some(crate::types::system_from_string("be helpful")),
            tools: Some(vec![ToolDefinition {
                name: "weather".to_string(),
                description: Some("Get weather".to_string()),
                input_schema: json!({"type": "object"}),
            }]),
            tool_choice: Some(ToolChoice::Auto),
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::xai(),
    );

    assert_eq!(payload["messages"][0]["role"], json!("system"));
    assert_eq!(payload["messages"][1]["role"], json!("user"));
    assert_eq!(payload["messages"][2]["role"], json!("tool"));
    assert_eq!(payload["tools"][0]["type"], json!("function"));
    assert_eq!(payload["tool_choice"], json!("auto"));
}

/// The system message carries the shared non-Anthropic identity override so an
/// OpenAI-compatible-served model (xAI here) does not introduce itself as
/// Claude, and the original Claude-authored body is preserved beneath it.
#[test]
fn system_message_carries_identity_override_for_first_party_provider() {
    let mut request = effort_request("grok-3", None);
    request.system = Some(crate::types::system_from_string(
        "You are Claude Code, Anthropic's official CLI for Claude.",
    ));
    let payload = build_chat_completion_request(&request, OpenAiCompatConfig::xai());
    let content = payload["messages"][0]["content"]
        .as_str()
        .expect("system content is a string");
    assert!(
        content.contains("You are grok-3, a large language model made by xAI"),
        "xAI identity override present: {content}"
    );
    assert!(content.contains("do not claim to be Claude or to be made by Anthropic"));
    assert!(
        content.contains("You are Claude Code, Anthropic's official CLI"),
        "original body preserved: {content}"
    );
}

/// A user-defined (custom) OpenAI-compatible provider has no known maker, so the
/// override still corrects the Claude claim but uses neutral wording rather than
/// mislabeling the model's origin.
#[test]
fn system_message_identity_override_is_neutral_for_custom_provider() {
    let custom = OpenAiCompatConfig::from_user("LM Studio", "http://localhost:1234/v1", None, true);
    let mut request = effort_request("local-model", None);
    request.system = Some(crate::types::system_from_string("Operating manual."));
    let payload = build_chat_completion_request(&request, custom);
    let content = payload["messages"][0]["content"]
        .as_str()
        .expect("system content is a string");
    assert!(
        content.contains("You are local-model, a large language model, operating"),
        "neutral identity override present: {content}"
    );
    assert!(
        !content.contains("made by OpenAI")
            && !content.contains("made by xAI")
            && !content.contains("made by Google"),
        "custom provider must not get a false maker: {content}"
    );
    assert!(content.contains("do not claim to be Claude"));
}

#[test]
fn request_translation_preserves_assistant_tool_call_arguments() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "grok-3".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage {
                role: "assistant".to_string(),
                content: vec![InputContentBlock::ToolUse {
                    id: "tool_42".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "src/main.rs"}),
                                    cache_control: None,
                }],
                thought_signature: None,
                reasoning_replay: None,
            }],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::xai(),
    );

    assert_eq!(payload["messages"][0]["role"], json!("assistant"));
    assert_eq!(
        payload["messages"][0]["tool_calls"][0]["id"],
        json!("tool_42")
    );
    assert_eq!(
        payload["messages"][0]["tool_calls"][0]["function"]["name"],
        json!("read_file")
    );
    assert_eq!(
        payload["messages"][0]["tool_calls"][0]["function"]["arguments"],
        json!(r#"{"path":"src/main.rs"}"#)
    );
}

#[test]
fn request_translation_preserves_mixed_assistant_text_and_multiple_tool_calls() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "grok-3".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage {
                role: "assistant".to_string(),
                content: vec![
                    InputContentBlock::Text {
                        text: "I'll inspect both files.".to_string(),
                        cache_control: None,
                    },
                    InputContentBlock::ToolUse {
                        id: "tool_a".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "a.rs"}),
                                            cache_control: None,
                    },
                    InputContentBlock::ToolUse {
                        id: "tool_b".to_string(),
                        name: "grep_search".to_string(),
                        input: json!({"pattern": "TODO"}),
                                            cache_control: None,
                    },
                ],
                thought_signature: None,
                reasoning_replay: None,
            }],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::xai(),
    );

    let message = &payload["messages"][0];
    assert_eq!(message["role"], json!("assistant"));
    assert_eq!(message["content"], json!("I'll inspect both files."));
    assert_eq!(message["tool_calls"][0]["id"], json!("tool_a"));
    assert_eq!(
        message["tool_calls"][0]["function"],
        json!({"name": "read_file", "arguments": "{\"path\":\"a.rs\"}"})
    );
    assert_eq!(message["tool_calls"][1]["id"], json!("tool_b"));
    assert_eq!(
        message["tool_calls"][1]["function"],
        json!({"name": "grep_search", "arguments": "{\"pattern\":\"TODO\"}"})
    );
}

/// A text-only assistant turn must omit `tool_calls` entirely: OpenAI-compatible
/// servers such as `DeepSeek` reject a present-but-empty `tool_calls` array with a
/// 400 ("Expected an array with minimum length 1, but got an empty array").
#[test]
fn request_translation_omits_tool_calls_for_text_only_assistant_message() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "deepseek-chat".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage {
                role: "assistant".to_string(),
                content: vec![InputContentBlock::Text {
                    text: "Hello there!".to_string(),
                    cache_control: None,
                }],
                thought_signature: None,
                reasoning_replay: None,
            }],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::from_user(
            "deepseek",
            "https://api.deepseek.com",
            Some("DEEPSEEK_API_KEY"),
            true,
        ),
    );

    let message = &payload["messages"][0];
    assert_eq!(message["role"], json!("assistant"));
    assert_eq!(message["content"], json!("Hello there!"));
    assert!(
        message.get("tool_calls").is_none(),
        "text-only assistant message must omit tool_calls, got: {message}"
    );
}

#[test]
fn request_translation_splits_user_text_around_ordered_tool_results() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "grok-3".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage {
                role: "user".to_string(),
                content: vec![
                    InputContentBlock::Text {
                        text: "before".to_string(),
                        cache_control: None,
                    },
                    InputContentBlock::ToolResult {
                        tool_use_id: "tool_a".to_string(),
                        content: vec![ToolResultContentBlock::Text {
                            text: "ok".to_string(),
                        }],
                        is_error: false,
                                            cache_control: None,
                    },
                    InputContentBlock::ToolResult {
                        tool_use_id: "tool_b".to_string(),
                        content: vec![
                            ToolResultContentBlock::Json {
                                value: json!({"failed": true}),
                            },
                            ToolResultContentBlock::Image {
                                source: ImageSource {
                                    kind: "base64".to_string(),
                                    media_type: "image/png".to_string(),
                                    data: "abc123".to_string(),
                                },
                            },
                        ],
                        is_error: true,
                                            cache_control: None,
                    },
                    InputContentBlock::Text {
                        text: "after".to_string(),
                        cache_control: None,
                    },
                ],
                thought_signature: None,
                reasoning_replay: None,
            }],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::xai(),
    );

    let messages = payload["messages"].as_array().expect("messages array");
    assert_eq!(messages[0], json!({"role": "user", "content": "before"}));
    assert_eq!(messages[1]["role"], json!("tool"));
    assert_eq!(messages[1]["tool_call_id"], json!("tool_a"));
    assert_eq!(messages[1]["content"], json!("ok"));
    assert!(messages[1].get("is_error").is_none());
    assert_eq!(messages[2]["role"], json!("tool"));
    assert_eq!(messages[2]["tool_call_id"], json!("tool_b"));
    assert_eq!(
        messages[2]["content"],
        json!("{\"failed\":true}\n[image image/png]")
    );
    assert!(messages[2].get("is_error").is_none());
    assert_eq!(messages[3], json!({"role": "user", "content": "after"}));
}

#[test]
fn request_translation_preserves_user_image_blocks() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "gpt-5.6-luna".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage::user_with_images(
                "inspect this",
                vec![ImageSource {
                    kind: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "abc123".to_string(),
                }],
            )],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::openai(),
    );

    assert_eq!(payload["messages"][0]["role"], json!("user"));
    assert_eq!(
        payload["messages"][0]["content"][0]["type"],
        json!("image_url")
    );
    assert_eq!(
        payload["messages"][0]["content"][0]["image_url"]["url"],
        json!("data:image/png;base64,abc123")
    );
    assert_eq!(payload["messages"][0]["content"][1]["type"], json!("text"));
    assert_eq!(
        payload["messages"][0]["content"][1]["text"],
        json!("inspect this")
    );
}

#[test]
fn vision_less_provider_degrades_images_to_text_placeholder() {
    // A DeepSeek-style endpoint rejects the `image_url` content variant with a
    // hard 400 ("unknown variant image_url, expected text"). With
    // `supports_vision` off, the image must be lowered to a text placeholder
    // instead of an `image_url` part, and the request must contain no `image_url`
    // anywhere.
    let mut config = OpenAiCompatConfig::from_user("DeepSeek", "https://api.deepseek.com/v1", None, false);
    assert!(!config.supports_vision, "custom providers default vision off");
    config.supports_vision = false;

    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "deepseek-chat".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage::user_with_images(
                "inspect this",
                vec![ImageSource {
                    kind: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "abc123".to_string(),
                }],
            )],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        config,
    );

    let serialized = serde_json::to_string(&payload).unwrap();
    assert!(
        !serialized.contains("image_url"),
        "vision-less request must not carry any image_url part: {serialized}"
    );
    // The text block and the image placeholder both survive as plain user text.
    let messages = payload["messages"].as_array().unwrap();
    let user_text: Vec<&str> = messages
        .iter()
        .filter(|m| m["role"] == json!("user"))
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert!(
        user_text.iter().any(|t| t.contains("inspect this")),
        "user text preserved: {user_text:?}"
    );
    assert!(
        user_text
            .iter()
            .any(|t| t.contains("image omitted") && t.contains("image/png")),
        "image degraded to a labeled placeholder: {user_text:?}"
    );
}

#[test]
fn vision_less_provider_degrade_is_deterministic() {
    // Same request + config must serialize byte-identically every call — the
    // DeepSeek image-degrade gate is a pure function of its inputs, so two
    // independent builds must never diverge.
    let mut config =
        OpenAiCompatConfig::from_user("DeepSeek", "https://api.deepseek.com/v1", None, false);
    config.supports_vision = false;

    let request = MessageRequest {
        model: "deepseek-chat".to_string(),
        max_tokens: 64,
        messages: vec![InputMessage::user_with_images(
            "inspect this",
            vec![ImageSource {
                kind: "base64".to_string(),
                media_type: "image/png".to_string(),
                data: "abc123".to_string(),
            }],
        )],
        system: None,
        tools: None,
        tool_choice: None,
        stream: false,
        thinking: None,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    };

    let first = build_chat_completion_request(&request, config);
    let second = build_chat_completion_request(&request, config);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "same request + config must build byte-identical payloads every call"
    );
}

#[test]
fn custom_openai_base_url_degrades_images_to_text_placeholder() {
    // `OPENAI_BASE_URL` is an escape hatch to any OpenAI-compatible server. Even
    // though the static OpenAI config supports vision, a non-official base URL
    // must use the same safe default as custom providers so vision-less servers
    // do not hard-400 on `image_url` parts.
    let config = super::request_config_for_base_url(
        OpenAiCompatConfig::openai(),
        "https://deepseek.example/v1",
    );
    assert!(
        !config.supports_vision,
        "custom OpenAI base URLs default vision off"
    );

    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "deepseek-chat".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage::user_with_images(
                "inspect this",
                vec![ImageSource {
                    kind: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "abc123".to_string(),
                }],
            )],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        config,
    );

    let serialized = serde_json::to_string(&payload).unwrap();
    assert!(
        !serialized.contains("image_url"),
        "custom OpenAI base URL must not carry image_url parts: {serialized}"
    );
    let messages = payload["messages"].as_array().unwrap();
    let user_text: Vec<&str> = messages
        .iter()
        .filter(|m| m["role"] == json!("user"))
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert!(
        user_text.iter().any(|t| t.contains("inspect this")),
        "user text preserved: {user_text:?}"
    );
    assert!(
        user_text
            .iter()
            .any(|t| t.contains("image omitted") && t.contains("image/png")),
        "image degraded to a labeled placeholder: {user_text:?}"
    );
}

#[test]
fn official_openai_base_url_keeps_image_url() {
    let config = super::request_config_for_base_url(
        OpenAiCompatConfig::openai(),
        DEFAULT_OPENAI_BASE_URL,
    );
    assert!(config.supports_vision, "official OpenAI keeps vision enabled");

    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "gpt-4o".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage::user_with_images(
                "inspect this",
                vec![ImageSource {
                    kind: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "abc123".to_string(),
                }],
            )],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        config,
    );

    assert_eq!(
        payload["messages"][0]["content"][0]["type"],
        json!("image_url")
    );
}

#[test]
fn vision_capable_custom_provider_keeps_image_url() {
    // A custom endpoint that opts into `supports_vision` still lowers images to
    // the multimodal `image_url` part — the degrade path is gated, not global.
    let mut config =
        OpenAiCompatConfig::from_user("VLM", "https://vlm.example/v1", None, false);
    config.supports_vision = true;

    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "vlm-1".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage::user_with_images(
                "inspect this",
                vec![ImageSource {
                    kind: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "abc123".to_string(),
                }],
            )],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        config,
    );

    assert_eq!(
        payload["messages"][0]["content"][0]["type"],
        json!("image_url")
    );
}

#[test]
fn openai_streaming_requests_include_usage_opt_in() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "gpt-5.6-sol".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage::user_text("hello")],
            system: None,
            tools: None,
            tool_choice: None,
            stream: true,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::openai(),
    );

    assert_eq!(payload["stream_options"], json!({"include_usage": true}));
}

#[test]
fn openai_supported_gpt_models_use_cache_key_without_unverified_retention() {
    for model in [
        "gpt-5.5",
        "gpt-5.5-fast",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.3-codex-spark",
    ] {
        let payload = build_chat_completion_request(
            &MessageRequest {
                model: model.to_string(),
                max_tokens: 64,
                messages: vec![InputMessage::user_text("dynamic tail")],
                system: Some(crate::types::system_from_string("stable system prefix")),
                tools: None,
                tool_choice: None,
                stream: true,
                thinking: None,
                output_config: None,
                effort: None,
                effort_band_ceiling: None,
            },
            OpenAiCompatConfig::openai(),
        );

        let key = payload["prompt_cache_key"]
            .as_str()
            .expect("prompt cache key should be present");
        assert!(key.starts_with("zo-"), "{model}");
        assert!(key.len() <= 64, "{model}");
        assert!(payload.get("prompt_cache_retention").is_none(), "{model}");
    }
}

#[test]
fn compatible_providers_skip_openai_prompt_cache_controls() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "grok-3".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage::user_text("hello")],
            system: Some(crate::types::system_from_string("stable system prefix")),
            tools: None,
            tool_choice: None,
            stream: true,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::xai(),
    );

    assert!(payload.get("prompt_cache_key").is_none());
    assert!(payload.get("prompt_cache_retention").is_none());
}

#[test]
fn xai_streaming_requests_skip_openai_specific_usage_opt_in() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "grok-3".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage::user_text("hello")],
            system: None,
            tools: None,
            tool_choice: None,
            stream: true,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::xai(),
    );

    assert!(payload.get("stream_options").is_none());
}

#[test]
fn user_endpoint_config_controls_stream_usage_opt_in() {
    let request = MessageRequest {
        model: "local-model".to_string(),
        max_tokens: 64,
        messages: vec![InputMessage::user_text("hello")],
        system: None,
        tools: None,
        tool_choice: None,
        stream: true,
        thinking: None,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    };

    let usage_enabled =
        OpenAiCompatConfig::from_user("LM Studio", "http://localhost:1234/v1", None, true);
    let payload = build_chat_completion_request(&request, usage_enabled);
    assert_eq!(payload["stream_options"], json!({"include_usage": true}));
    assert_eq!(usage_enabled.provider_name, "LM Studio");
    assert_eq!(usage_enabled.api_key_env, "");
    assert_eq!(usage_enabled.default_base_url, "http://localhost:1234/v1");

    let usage_disabled = OpenAiCompatConfig::from_user(
        "LocalAI",
        "http://localhost:8080/v1",
        Some("LOCALAI_API_KEY"),
        false,
    );
    let payload = build_chat_completion_request(&request, usage_disabled);
    assert!(payload.get("stream_options").is_none());
    assert_eq!(usage_disabled.api_key_env, "LOCALAI_API_KEY");
}

fn effort_request(model: &str, budget_tokens: Option<u32>) -> MessageRequest {
    MessageRequest {
        model: model.to_string(),
        max_tokens: 1234,
        messages: vec![InputMessage::user_text("hi")],
        system: None,
        tools: None,
        tool_choice: None,
        stream: false,
        thinking: budget_tokens.map(crate::types::ThinkingConfig::enabled),
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    }
}

#[test]
fn reasoning_model_uses_completion_tokens_and_effort() {
    let payload = build_chat_completion_request(
        &effort_request("gpt-5.6-luna", Some(24_000)),
        OpenAiCompatConfig::openai(),
    );
    // Reasoning models reject `max_tokens`; the budget must reach them as
    // `reasoning_effort` + `max_completion_tokens`. The budget fallback uses
    // the same GPT wire projection as explicit effort.
    assert!(payload.get("max_tokens").is_none());
    assert_eq!(payload["max_completion_tokens"], json!(1234));
    assert_eq!(payload["reasoning_effort"], json!("max"));
}

#[test]
fn gpt55_fast_clamps_max_like_gpt55() {
    // `/fast` is a serving-priority signal, not a reasoning-effort ceiling. For
    // legacy GPT families, Max still clamps to xhigh.
    for model in ["gpt-5.5", "gpt-5.5-fast"] {
        let payload = build_chat_completion_request(
            &effort_request(model, Some(24_000)),
            OpenAiCompatConfig::openai(),
        );
        assert_eq!(
            payload["reasoning_effort"],
            json!("xhigh"),
            "{model} should clamp max budgets to xhigh"
        );
        assert_eq!(payload["max_completion_tokens"], json!(1234));
    }
}

#[test]
fn reasoning_model_without_thinking_still_drops_max_tokens() {
    let payload = build_chat_completion_request(
        &effort_request("gpt-5.6-sol", None),
        OpenAiCompatConfig::openai(),
    );
    assert!(payload.get("max_tokens").is_none());
    assert_eq!(payload["max_completion_tokens"], json!(1234));
    assert!(payload.get("reasoning_effort").is_none());
}

#[test]
fn reasoning_model_zero_budget_omits_reasoning_effort() {
    let payload = build_chat_completion_request(
        &effort_request("gpt-5.6-sol", Some(0)),
        OpenAiCompatConfig::openai(),
    );
    assert!(payload.get("max_tokens").is_none());
    assert_eq!(payload["max_completion_tokens"], json!(1234));
    assert!(payload.get("reasoning_effort").is_none());
}

#[test]
fn custom_provider_opt_in_sends_reasoning_effort_for_an_unrecognized_model() {
    // A gateway-served reasoning model matches none of the OpenAI id families
    // `is_reasoning_model` knows, so without the opt-in its requests carried NO
    // effort field at all — `/effort` (and the Smart dynamic band) were inert
    // for it. The opt-in must add `reasoning_effort` WITHOUT switching the
    // token field: `max_completion_tokens` is an OpenAI-reasoning wire fact and
    // a compatible gateway may only accept `max_tokens`.
    let mut config = OpenAiCompatConfig::from_user(
        "agent",
        "https://agentrouter.org/v1",
        Some("ZO_AGENT_API_KEY"),
        false,
    );
    config.supports_reasoning_effort = true;

    let payload = build_chat_completion_request(&effort_request("kimi-k3", Some(24_000)), config);
    assert_eq!(payload["max_tokens"], json!(1234));
    assert!(payload.get("max_completion_tokens").is_none());
    assert_eq!(payload["reasoning_effort"], json!("xhigh"));
}

#[test]
fn custom_provider_without_opt_in_still_omits_reasoning_effort() {
    // Default stays off: an endpoint that does not take the field would 400 on
    // it, so the operator must declare the capability.
    let config = OpenAiCompatConfig::from_user(
        "agent",
        "https://agentrouter.org/v1",
        Some("ZO_AGENT_API_KEY"),
        false,
    );
    let payload = build_chat_completion_request(&effort_request("kimi-k3", Some(24_000)), config);
    assert_eq!(payload["max_tokens"], json!(1234));
    assert!(payload.get("reasoning_effort").is_none());
}

#[test]
fn custom_provider_opt_in_resolves_the_dynamic_band_per_request() {
    // The whole point of wiring the field through: on an unrecognized model the
    // band stays a real range instead of collapsing onto one rung, and the
    // per-request difficulty signals actually move the value that ships.
    //
    // The band is exercised at its LOW end, which is where the wire vocabulary
    // can still tell the rungs apart: the OpenAI projection maps everything
    // above `xhigh` (internal `Max`/`Ultra`) onto `"xhigh"`, so a heavy turn's
    // extra rungs are invisible on the wire while a simple turn's downshift is
    // not. That low end is exactly what the Smart complexity floor drives —
    // `Trivial` floors at `low`, `Small` at `medium`
    // (`smart_settings::smart_turn_effort_band_for_complexity`).
    let mut config = OpenAiCompatConfig::from_user(
        "agent",
        "https://agentrouter.org/v1",
        Some("ZO_AGENT_API_KEY"),
        false,
    );
    config.supports_reasoning_effort = true;

    let band = |text: &str, floor| {
        let mut request = effort_request("kimi-k3", None);
        request.messages = vec![InputMessage::user_text(text)];
        request.effort = Some(floor);
        request.effort_band_ceiling = Some(crate::types::EffortLevel::Xhigh);
        build_chat_completion_request(&request, config)["reasoning_effort"].clone()
    };

    // A trivial turn's `low` floor with no difficulty signals ships as `low` —
    // before the band-clamp fix every one of these shipped `high`.
    assert_eq!(band("hi", crate::types::EffortLevel::Low), json!("low"));
    // One signal (heavy intent) steps exactly one rung up from that floor.
    assert_eq!(
        band("이 아키텍처를 분석해줘", crate::types::EffortLevel::Low),
        json!("medium")
    );
    // A saturated turn climbs to the requested ceiling.
    let mut saturated = effort_request("kimi-k3", None);
    saturated.messages = vec![InputMessage::user_text(format!(
        "이 아키텍처를 분석해줘 {}",
        "상세히 ".repeat(300)
    ))];
    saturated.effort = Some(crate::types::EffortLevel::Low);
    saturated.effort_band_ceiling = Some(crate::types::EffortLevel::Xhigh);
    assert_eq!(
        build_chat_completion_request(&saturated, config)["reasoning_effort"],
        json!("xhigh")
    );
}

#[test]
fn non_reasoning_model_keeps_max_tokens_and_no_effort() {
    let payload = build_chat_completion_request(
        &effort_request("gpt-4o", Some(24_000)),
        OpenAiCompatConfig::openai(),
    );
    assert_eq!(payload["max_tokens"], json!(1234));
    assert!(payload.get("max_completion_tokens").is_none());
    assert!(payload.get("reasoning_effort").is_none());
}

#[test]
fn reasoning_effort_maps_zo_tiers() {
    // Budget-derived tiers match the shared fallback thresholds. This
    // budget-only fallback has no Ultra bucket, so every budget at or above
    // the Max threshold (24k) lands in the same internal bucket — which now
    // reaches the wire as `max` on GPT-5.6 and clamps to `xhigh` only on the
    // families that predate the rung.
    assert_eq!(reasoning_effort_for("gpt-5.5", 1_024), "low");
    assert_eq!(reasoning_effort_for("gpt-5.5", 4_096), "medium");
    assert_eq!(reasoning_effort_for("gpt-5.5", 10_000), "high");
    assert_eq!(reasoning_effort_for("gpt-5.5", 16_000), "xhigh");
    assert_eq!(reasoning_effort_for("gpt-5.5", 20_000), "xhigh");
    assert_eq!(reasoning_effort_for("gpt-5.5", 24_000), "xhigh");
    assert_eq!(reasoning_effort_for("gpt-5.5", 32_000), "xhigh");
    // GPT fast is service priority, not a reasoning-effort ceiling; Codex Spark
    // and legacy fast aliases keep the same conservative Max -> xhigh clamp.
    assert_eq!(reasoning_effort_for("gpt-5.6-sol", 16_000), "xhigh");
    assert_eq!(reasoning_effort_for("gpt-5.6-sol", 24_000), "max");
    assert_eq!(reasoning_effort_for("gpt-5.6-terra", 32_000), "max");
    assert_eq!(reasoning_effort_for("gpt-5.6-luna", 32_000), "max");
    assert_eq!(reasoning_effort_for("gpt-5.5-fast", 32_000), "xhigh");
    assert_eq!(reasoning_effort_for("gpt-5.3-codex-spark", 24_000), "xhigh");
}

#[test]
fn reasoning_model_honors_explicit_request_effort() {
    // The headless benchmark path sets request.effort (derived from ZO_EFFORT),
    // which takes priority over the thinking budget (mod.rs: `if let Some(level) =
    // request.effort`). The other effort tests leave effort: None and exercise the
    // budget fallback; this pins the production path → reasoning_effort wire
    // string. `max` is a real GPT-5.6 value; `ultra` never is, so it lands on
    // whatever top rung the model actually exposes.
    for (model, top) in [
        ("gpt-5.5", "xhigh"),
        ("gpt-5.5-fast", "xhigh"),
        ("gpt-5.3-codex-spark", "xhigh"),
        ("gpt-5.6-sol", "max"),
        ("gpt-5.6-terra", "max"),
        ("gpt-5.6-luna", "max"),
    ] {
        for (level, expected) in [
            (crate::types::EffortLevel::Low, "low"),
            (crate::types::EffortLevel::Medium, "medium"),
            (crate::types::EffortLevel::High, "high"),
            (crate::types::EffortLevel::Xhigh, "xhigh"),
            (crate::types::EffortLevel::Max, top),
            (crate::types::EffortLevel::Ultra, top),
        ] {
            let mut req = effort_request(model, Some(24_000));
            req.effort = Some(level);
            let payload = build_chat_completion_request(&req, OpenAiCompatConfig::openai());
            assert_eq!(
                payload["reasoning_effort"],
                json!(expected),
                "{model} request.effort {level:?} should override the thinking budget"
            );
        }
    }
}

#[test]
fn openai_compat_projects_internal_ultra_to_the_model_top_rung() {
    // `ultra` is a ChatGPT product mode, not a wire value — it must land on the
    // model's real ceiling, which is `max` from GPT-5.6 on.
    for (model, expected) in [
        ("gpt-5.6-sol-2026-07-09", "max"),
        ("gpt-5.6-terra@openai", "max"),
        ("gpt-5.6-luna", "max"),
        ("gpt-5.5", "xhigh"),
    ] {
        let mut req = effort_request(model, Some(20_000));
        req.effort = Some(crate::types::EffortLevel::Ultra);
        let payload = build_chat_completion_request(&req, OpenAiCompatConfig::openai());
        assert_eq!(payload["reasoning_effort"], json!(expected), "{model}");
    }
}

#[test]
fn banded_request_projects_resolved_internal_rungs_to_supported_wire_value() {
    // The second OpenAI wire path (API-key/custom OpenAI-compatible providers)
    // must resolve the dynamic ultra band identically to the ChatGPT-
    // subscription path (chatgpt_backend) — no split-brain for a sol reached
    // via a custom provider.
    let mut trivial = effort_request("gpt-5.6-sol", None);
    trivial.effort = Some(crate::types::EffortLevel::Xhigh);
    trivial.effort_band_ceiling = Some(crate::types::EffortLevel::Max);
    let payload = build_chat_completion_request(&trivial, OpenAiCompatConfig::openai());
    assert_eq!(payload["reasoning_effort"], json!("xhigh"));

    let mut heavy = effort_request("gpt-5.6-sol", None);
    heavy.messages = vec![InputMessage::user_text("please refactor this module")];
    heavy.effort = Some(crate::types::EffortLevel::Xhigh);
    heavy.effort_band_ceiling = Some(crate::types::EffortLevel::Max);
    let payload = build_chat_completion_request(&heavy, OpenAiCompatConfig::openai());
    assert_eq!(payload["reasoning_effort"], json!("max"));
}

#[test]
fn banded_request_resolves_the_same_rung_through_chatgpt_backend_and_openai_compat() {
    // Cross-backend parity tripwire: an identical banded MessageRequest must
    // resolve to the same supported wire value whether it travels the ChatGPT-
    // subscription Responses path or the API-key/custom OpenAI-compatible
    // Chat Completions path — both call the ONE shared resolver.
    let cases: &[(&str, &str)] = &[("hi", "xhigh"), ("please refactor this module", "max")];
    for (text, expected) in cases {
        let mut request = effort_request("gpt-5.6-sol", None);
        request.messages = vec![InputMessage::user_text(*text)];
        request.effort = Some(crate::types::EffortLevel::Xhigh);
        request.effort_band_ceiling = Some(crate::types::EffortLevel::Max);

        let responses_body =
            crate::providers::chatgpt_backend::build_responses_request(&request, "i", true);
        let chat_completions_body =
            build_chat_completion_request(&request, OpenAiCompatConfig::openai());

        assert_eq!(responses_body["reasoning"]["effort"], json!(*expected));
        assert_eq!(chat_completions_body["reasoning_effort"], json!(*expected));
        assert_eq!(
            responses_body["reasoning"]["effort"], chat_completions_body["reasoning_effort"],
            "text={text:?} must resolve to the same rung on both wire paths"
        );
    }
}

#[test]
fn tool_choice_translation_supports_required_function() {
    assert_eq!(openai_tool_choice(&ToolChoice::Any), json!("required"));
    assert_eq!(
        openai_tool_choice(&ToolChoice::Tool {
            name: "weather".to_string(),
        }),
        json!({"type": "function", "function": {"name": "weather"}})
    );
}

#[test]
fn parses_tool_arguments_fallback() {
    assert_eq!(
        parse_tool_arguments("{\"city\":\"Paris\"}"),
        json!({"city": "Paris"})
    );
    assert_eq!(parse_tool_arguments("not-json"), json!({"raw": "not-json"}));
}

#[test]
fn missing_xai_api_key_is_provider_specific() {
    let _lock = env_lock();
    std::env::remove_var("XAI_API_KEY");
    let error = OpenAiCompatClient::from_env(OpenAiCompatConfig::xai())
        .expect_err("missing key should error");
    assert!(matches!(
        error,
        ApiError::MissingCredentials {
            provider: "xAI",
            ..
        }
    ));
}

#[test]
fn endpoint_builder_accepts_base_urls_and_full_endpoints() {
    assert_eq!(
        chat_completions_endpoint("https://api.x.ai/v1"),
        "https://api.x.ai/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_endpoint("https://api.x.ai/v1/"),
        "https://api.x.ai/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_endpoint("https://api.x.ai/v1/chat/completions"),
        "https://api.x.ai/v1/chat/completions"
    );
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn normalizes_stop_reasons() {
    assert_eq!(normalize_finish_reason("stop"), "end_turn");
    assert_eq!(normalize_finish_reason("tool_calls"), "tool_use");
}

#[test]
fn next_sse_frame_preserves_remaining_buffer_for_lf_and_crlf_separators() {
    let mut lf = b"data: one\n\ndata: two\n\n".to_vec();
    let mut scanned = 0;
    assert_eq!(
        next_sse_frame(&mut lf, &mut scanned).as_deref(),
        Some("data: one")
    );
    assert_eq!(String::from_utf8(lf).expect("utf8"), "data: two\n\n");
    assert_eq!(scanned, 0);

    let mut crlf = b"data: one\r\n\r\ndata: two\r\n\r\n".to_vec();
    assert_eq!(
        next_sse_frame(&mut crlf, &mut scanned).as_deref(),
        Some("data: one")
    );
    assert_eq!(String::from_utf8(crlf).expect("utf8"), "data: two\r\n\r\n");
    assert_eq!(scanned, 0);
}

#[test]
fn parse_sse_frame_ignores_comments_and_joins_data_lines() {
    let frame = ": keep-alive\ndata: {\"id\":\"chunk\",\ndata: \"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n";
    let parsed = parse_sse_frame(frame).expect("frame should parse");
    assert!(parsed.is_some());
}

/// GLM / agentrouter-style gateways emit an explicit `null` for empty arrays
/// (`"tool_calls": null`, and even `"choices": null`) instead of omitting the
/// field. `#[serde(default)]` only covers an *absent* field, so before the
/// null-tolerant deserializer the stream aborted with
/// `invalid type: null, expected a sequence`. Both must degrade to empty.
#[test]
fn parse_sse_frame_tolerates_null_arrays_from_openai_compat_gateways() {
    let with_null_tool_calls =
        "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"hi\",\"tool_calls\":null}}]}\n";
    let parsed = parse_sse_frame(with_null_tool_calls)
        .expect("null delta.tool_calls must parse, not abort the stream")
        .expect("frame yields a chunk");
    assert_eq!(parsed.choices.len(), 1);
    assert!(
        parsed.choices[0].delta.tool_calls.is_empty(),
        "null tool_calls must degrade to no tool calls"
    );

    let with_null_choices = "data: {\"id\":\"c1\",\"choices\":null}\n";
    let parsed = parse_sse_frame(with_null_choices)
        .expect("null choices must parse, not abort the stream")
        .expect("frame yields a chunk");
    assert!(
        parsed.choices.is_empty(),
        "null choices must degrade to an empty list"
    );
}

/// Gateways commonly stamp `id` on the first chunk only, or omit it entirely.
/// While `id` was the one required chunk field, such a stream died on the first
/// delta with a "missing field id" JSON error. It must parse and still drive
/// the turn.
#[test]
fn parse_sse_frame_tolerates_chunks_without_an_id() {
    let frame = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n";
    let parsed = parse_sse_frame(frame)
        .expect("a chunk without id must not abort the stream")
        .expect("frame yields a chunk");
    assert!(parsed.id.is_none());

    let mut state = StreamState::new("kimi-k3".to_string(), OpenAiCompatConfig::openai());
    let events = state.ingest_chunk(parsed).expect("chunk ingests");
    assert!(
        events.iter().any(|event| matches!(
            event,
            StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                delta: ContentBlockDelta::TextDelta { text },
                ..
            }) if text == "hi"
        )),
        "an id-less chunk must still stream its text"
    );
}

/// A gateway that fails after the response headers are out sends
/// `{"error": {...}}` where a chunk belongs. Every chunk field is optional, so
/// without an explicit envelope check the payload would deserialize as an empty
/// chunk and the real failure would be dropped silently.
#[test]
fn parse_sse_frame_surfaces_mid_stream_error_envelopes() {
    let frame = "data: {\"error\":{\"message\":\"upstream is overloaded\",\"type\":\"rate_limit_exceeded\"}}\n";
    let error = parse_sse_frame(frame).expect_err("an error frame must fail the stream");
    assert!(
        matches!(
            &error,
            ApiError::StreamApi { error_type, message, retryable, .. }
                if error_type.as_deref() == Some("rate_limit_exceeded")
                    && message.as_deref() == Some("upstream is overloaded")
                    && *retryable
        ),
        "expected the gateway's own message, got {error:?}"
    );
}

/// Responses-API-shaped errors carry `code` where chat-completions carry
/// `type`, and a non-capacity failure must not be advertised as retryable.
#[test]
fn parse_sse_frame_reads_error_code_when_type_is_absent() {
    let frame = "data: {\"error\":{\"message\":\"context length exceeded\",\"code\":\"context_length_exceeded\"}}\n";
    let error = parse_sse_frame(frame).expect_err("an error frame must fail the stream");
    assert!(
        matches!(
            &error,
            ApiError::StreamApi { error_type, retryable, .. }
                if error_type.as_deref() == Some("context_length_exceeded") && !*retryable
        ),
        "expected code as the error type, got {error:?}"
    );
}

/// The envelope check is guarded by a cheap substring prefilter, so a content
/// delta that merely quotes the word must still parse as ordinary text.
#[test]
fn parse_sse_frame_keeps_content_deltas_that_mention_error() {
    let frame =
        "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"the \\\"error\\\" is here\"}}]}\n";
    let parsed = parse_sse_frame(frame)
        .expect("content quoting the word must not be treated as a failure")
        .expect("frame yields a chunk");
    assert_eq!(parsed.choices.len(), 1);
}

#[test]
fn sse_parser_large_frame_split_across_many_chunks_parses_once() {
    let big = "x".repeat(512 * 1024);
    let frame =
        format!("data: {{\"id\":\"chunk\",\"choices\":[{{\"delta\":{{\"content\":\"{big}\"}}}}]}}");
    let bytes = frame.as_bytes();
    let mut parser = OpenAiSseParser::new();
    let mut offset = 0;

    while offset < bytes.len() {
        let end = (offset + 1024).min(bytes.len());
        assert!(
            parser
                .push(&bytes[offset..end])
                .expect("partial push")
                .is_empty(),
            "unterminated partial frame must not emit"
        );
        offset = end;
    }

    let events = parser.push(b"\n\n").expect("terminator push");
    assert_eq!(
        events.len(),
        1,
        "frame must parse exactly once after terminator"
    );
    assert_eq!(
        events[0].choices[0].delta.content.as_deref(),
        Some(big.as_str())
    );
}

#[test]
fn sse_parser_separator_split_across_chunks_is_found() {
    let mut parser = OpenAiSseParser::new();
    assert!(
        parser
            .push(b"data: {\"id\":\"chunk\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}")
            .expect("partial")
            .is_empty()
    );
    assert!(parser.push(b"\r").expect("cr").is_empty());
    assert!(parser.push(b"\n").expect("lf").is_empty());
    assert!(parser.push(b"\r").expect("cr").is_empty());

    let events = parser.push(b"\n").expect("terminator");
    assert_eq!(events.len(), 1, "split CRLFCRLF separator must be detected");
    assert_eq!(events[0].choices[0].delta.content.as_deref(), Some("hi"));
}

#[test]
fn sse_parser_rejects_unterminated_frame_past_the_shared_cap() {
    // The parser shares the crate-wide SSE buffer cap via
    // `crate::sse::guard_sse_buffer_push`. A server that streams past the cap
    // without ever emitting a frame separator must be rejected rather than
    // buffered without bound. A single chunk of `cap + 1` bytes (no separator)
    // is the direct oversized case the parser previously had no test for.
    let mut parser = OpenAiSseParser::new();
    let oversized = vec![b'a'; crate::sse::MAX_SSE_BUFFER_BYTES + 1];
    let error = parser
        .push(&oversized)
        .expect_err("an over-cap unterminated frame must be rejected");
    assert!(
        matches!(error, ApiError::InvalidSseFrame(_)),
        "expected InvalidSseFrame, got {error:?}"
    );
}

#[test]
fn sse_parser_accepts_a_frame_that_fills_the_cap_then_terminates() {
    // The boundary the cap+1 rejection sits just past: a payload that brings the
    // retained buffer to exactly the cap, delivered in below-cap chunks and then
    // terminated, must parse — the guard rejects growth *past* the cap, not up to
    // it. Chunks stay under the cap so each individual `push` is admissible.
    let content_len = crate::sse::MAX_SSE_BUFFER_BYTES / 2;
    let big = "y".repeat(content_len);
    let frame =
        format!("data: {{\"id\":\"chunk\",\"choices\":[{{\"delta\":{{\"content\":\"{big}\"}}}}]}}");
    let bytes = frame.as_bytes();
    let mut parser = OpenAiSseParser::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let end = (offset + 64 * 1024).min(bytes.len());
        assert!(
            parser.push(&bytes[offset..end]).expect("partial push").is_empty(),
            "unterminated partial frame must not emit"
        );
        offset = end;
    }
    let events = parser.push(b"\n\n").expect("terminator push");
    assert_eq!(events.len(), 1, "a cap-fitting frame must parse once terminated");
    assert_eq!(events[0].choices[0].delta.content.as_deref(), Some(big.as_str()));
}

#[test]
fn request_translation_flattens_tool_results_without_dropping_order() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "grok-3".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage {
                role: "user".to_string(),
                content: vec![InputContentBlock::ToolResult {
                    tool_use_id: "tool_1".to_string(),
                    content: vec![
                        ToolResultContentBlock::Text {
                            text: "line one".to_string(),
                        },
                        ToolResultContentBlock::Json {
                            value: json!({"ok": true}),
                        },
                    ],
                    is_error: false,
                                    cache_control: None,
                }],
                thought_signature: None,
                reasoning_replay: None,
            }],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::xai(),
    );

    assert_eq!(payload["messages"][0]["role"], json!("tool"));
    assert_eq!(
        payload["messages"][0]["content"],
        json!("line one\n{\"ok\":true}")
    );
}

#[test]
fn normalize_response_reports_cache_only_when_provider_supports_it() {
    use super::{ChatCompletionResponse, normalize_response};

    let raw = json!({
        "id": "resp_1",
        "model": "gpt-5.6-sol",
        "choices": [{
            "message": { "role": "assistant", "content": "hi", "tool_calls": [] },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "prompt_tokens_details": { "cached_tokens": 40 }
        }
    });

    // OpenAI reports prompt-cache hits as `prompt_tokens_details.cached_tokens`;
    // surface that real figure instead of making the HUD look like cache missed.
    let response: ChatCompletionResponse =
        serde_json::from_value(raw.clone()).expect("valid response");
    let normalized =
        normalize_response("gpt-5.6-sol", response, OpenAiCompatConfig::openai()).expect("normalizes");
    assert_eq!(normalized.usage.input_tokens, 60);
    assert_eq!(normalized.usage.output_tokens, 20);
    assert_eq!(normalized.usage.cache_read_input_tokens, 40);
    assert_eq!(normalized.usage.total_tokens(), 120);

    // Providers that do not opt into cache reporting still avoid fabricating
    // cache hits from OpenAI-shaped but unsupported payloads.
    let cache_unaware = OpenAiCompatConfig {
        supports_cache_tokens: false,
        ..OpenAiCompatConfig::openai()
    };
    let response: ChatCompletionResponse = serde_json::from_value(raw).expect("valid response");
    let normalized = normalize_response("gpt-5.6-sol", response, cache_unaware).expect("normalizes");
    assert_eq!(normalized.usage.input_tokens, 100);
    assert_eq!(normalized.usage.cache_read_input_tokens, 0);
    assert_eq!(normalized.usage.total_tokens(), 120);
}

#[test]
fn reasoning_content_streams_as_thinking_then_settles_before_answer() {
    // DeepSeek-reasoner streams `reasoning_content` ahead of `content`. It must
    // surface as a Thinking block that settles (closes) the instant the answer
    // begins — never lingering open above live prose.
    let mut state = StreamState::new("deepseek-reasoner".to_string(), OpenAiCompatConfig::openai());

    let reasoning_chunk: ChatCompletionChunk = serde_json::from_value(json!({
        "id": "c1",
        "choices": [{ "delta": { "reasoning_content": "Let me think." } }]
    }))
    .unwrap();
    let r_events = state.ingest_chunk(reasoning_chunk).unwrap();
    assert!(
        r_events.iter().any(|event| matches!(
            event,
            StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index: OPENAI_REASONING_BLOCK_INDEX,
                content_block: OutputContentBlock::Thinking { .. },
            })
        )),
        "reasoning_content opens a Thinking block: {r_events:?}"
    );
    assert!(
        r_events.iter().any(|event| matches!(
            event,
            StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                index: OPENAI_REASONING_BLOCK_INDEX,
                delta: ContentBlockDelta::ThinkingDelta { .. },
            })
        )),
        "reasoning_content emits a thinking delta: {r_events:?}"
    );

    let content_chunk: ChatCompletionChunk = serde_json::from_value(json!({
        "id": "c1",
        "choices": [{ "delta": { "content": "The answer." } }]
    }))
    .unwrap();
    let c_events = state.ingest_chunk(content_chunk).unwrap();
    assert!(
        c_events.iter().any(|event| matches!(
            event,
            StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index: OPENAI_REASONING_BLOCK_INDEX
            })
        )),
        "reasoning block closes when the answer begins: {c_events:?}"
    );
    assert!(
        c_events.iter().any(|event| matches!(
            event,
            StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index: 0,
                content_block: OutputContentBlock::Text { .. },
            })
        )),
        "answer text block opens after reasoning: {c_events:?}"
    );

    // A late reasoning delta after the answer started must be dropped (no reopen,
    // no thinking delta) — otherwise the reasoning block height flips above prose.
    let late_chunk: ChatCompletionChunk = serde_json::from_value(json!({
        "id": "c1",
        "choices": [{ "delta": { "reasoning_content": "(stray late thought)" } }]
    }))
    .unwrap();
    let late_events = state.ingest_chunk(late_chunk).unwrap();
    assert!(
        late_events.iter().all(|event| !matches!(
            event,
            StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index: OPENAI_REASONING_BLOCK_INDEX,
                ..
            }) | StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                index: OPENAI_REASONING_BLOCK_INDEX,
                ..
            })
        )),
        "late reasoning after the answer must be dropped: {late_events:?}"
    );
}

#[test]
fn plain_content_stream_emits_no_thinking_block() {
    // A non-reasoning provider (gpt-compat / grok) sends no `reasoning_content`,
    // so nothing changes: no Thinking block, just the answer text.
    let mut state = StreamState::new("gpt-4o".to_string(), OpenAiCompatConfig::openai());
    let chunk: ChatCompletionChunk = serde_json::from_value(json!({
        "id": "c1",
        "choices": [{ "delta": { "content": "hi" } }]
    }))
    .unwrap();
    let events = state.ingest_chunk(chunk).unwrap();
    assert!(
        events.iter().all(|event| !matches!(
            event,
            StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                content_block: OutputContentBlock::Thinking { .. },
                ..
            })
        )),
        "no Thinking block without reasoning_content: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// Mid-stream restart: pre-commit stalls recover; post-commit stalls propagate.
// ---------------------------------------------------------------------------

struct EnvVarGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let original = std::env::var_os(key);
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn streaming_request() -> MessageRequest {
    MessageRequest {
        model: "gpt-4o".to_string(),
        max_tokens: 128,
        messages: vec![InputMessage::user_text("hi")],
        system: None,
        tools: None,
        tool_choice: None,
        stream: true,
        thinking: None,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    }
}


#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn openai_compat_stalled_precommit_stream_restarts_and_recovers() {
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
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;

        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        let body = concat!(
            "data: {\"id\":\"c2\",\"choices\":[{\"delta\":{\"content\":\"recovered\"}}]}\n\n",
            "data: {\"id\":\"c2\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],",
            "\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n"
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    });

    let _idle = EnvVarGuard::set(crate::providers::STREAM_IDLE_TIMEOUT_ENV, Some("300"));
    let client = OpenAiCompatClient::new("token", OpenAiCompatConfig::openai())
        .with_base_url(format!("http://{addr}/v1"))
        .with_retry_policy(
            3,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );
    let mut stream = client
        .stream_message(&streaming_request())
        .await
        .expect("open stream");

    let mut text = String::new();
    while let Some(event) = stream.next_event().await.expect("restart should recover") {
        if let StreamEvent::ContentBlockDelta(delta) = &event {
            if let ContentBlockDelta::TextDelta { text: chunk } = &delta.delta {
                text.push_str(chunk);
            }
        }
    }
    server.await.unwrap();

    assert_eq!(text, "recovered");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// A pre-commit `{"error": {...}}` frame must be re-opened, not surfaced.
///
/// This is the sibling of the Anthropic defect: a gateway that dies after the
/// response headers are out sends the error where a chunk belongs, so the failure
/// arrives from the SSE parser rather than the body read — and the parser's `?`
/// used to bypass the restart gate that exists for exactly this case. The lane
/// matters because it is where a quota escape LANDS (gpt-5.6 and every custom
/// provider), so a capacity dip there would have killed the very turn the escape
/// was rescuing.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn openai_compat_precommit_error_frame_restarts_and_recovers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        let mut scratch = [0u8; 1024];
        // First attempt: a capacity error frame on an HTTP 200 stream, before any
        // content — the shape the restart gate is supposed to absorb invisibly.
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = first.read(&mut scratch).await;
        let error_body =
            "data: {\"error\":{\"message\":\"upstream is overloaded\",\"type\":\"rate_limit_exceeded\"}}\n\n";
        // `connection: close` matters: a COMPLETE response (content-length met)
        // would otherwise go back into reqwest's pool and the re-open would reuse
        // that socket instead of dialing, so the server's next `accept` would never
        // fire and the reopen would stall out its whole budget.
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            error_body.len()
        );
        first.write_all(head.as_bytes()).await.unwrap();
        first.write_all(error_body.as_bytes()).await.unwrap();
        first.flush().await.unwrap();

        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        let body = concat!(
            "data: {\"id\":\"c2\",\"choices\":[{\"delta\":{\"content\":\"recovered\"}}]}\n\n",
            "data: {\"id\":\"c2\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],",
            "\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n"
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    });

    let client = OpenAiCompatClient::new("token", OpenAiCompatConfig::openai())
        .with_base_url(format!("http://{addr}/v1"))
        .with_retry_policy(
            3,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );
    let mut stream = client
        .stream_message(&streaming_request())
        .await
        .expect("open stream");

    let mut text = String::new();
    while let Some(event) = stream
        .next_event()
        .await
        .expect("a pre-commit error frame must be re-opened, not surfaced")
    {
        if let StreamEvent::ContentBlockDelta(delta) = &event {
            if let ContentBlockDelta::TextDelta { text: chunk } = &delta.delta {
                text.push_str(chunk);
            }
        }
    }
    server.await.unwrap();

    assert_eq!(text, "recovered");
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the error frame must cost exactly one transparent re-open"
    );
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn openai_compat_restart_budget_exhausted_surfaces_error() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut conn, _) = listener.accept().await.unwrap();
            server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut scratch = [0u8; 1024];
            let _ = conn.read(&mut scratch).await;
            conn.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
                .await
                .unwrap();
            conn.flush().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        }
    });

    let _idle = EnvVarGuard::set(crate::providers::STREAM_IDLE_TIMEOUT_ENV, Some("300"));
    let client = OpenAiCompatClient::new("token", OpenAiCompatConfig::openai())
        .with_base_url(format!("http://{addr}/v1"))
        .with_retry_policy(
            1,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );
    let mut stream = client
        .stream_message(&streaming_request())
        .await
        .expect("open stream");

    let error = stream.next_event().await.expect_err("budget exhausted");
    assert!(
        matches!(
            error,
            ApiError::StreamApi { error_type, .. }
                if error_type.as_deref() == Some("stream_idle_timeout")
        ),
        "expected idle-timeout after restart budget exhausted"
    );
    server.await.unwrap();
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// Send one streaming request through a custom provider built from `provider`
/// JSON and return the raw HTTP request bytes the endpoint received.
async fn capture_custom_provider_request(provider: serde_json::Value) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = conn.read(&mut buf).await.unwrap();
        let captured = String::from_utf8_lossy(&buf[..n]).into_owned();
        conn.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
              data: {\"id\":\"c1\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        )
        .await
        .unwrap();
        conn.flush().await.unwrap();
        captured
    });

    let config: super::CustomProviderConfig = serde_json::from_value(provider).unwrap();
    let client = OpenAiCompatClient::new("token", config.to_static_config())
        .with_base_url(format!("http://{addr}/v1"));
    let _ = client
        .stream_message(&streaming_request())
        .await
        .expect("stream opens");
    server.await.unwrap()
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn custom_provider_presents_configured_client_fingerprint() {
    let _guard = env_lock();

    // Named preset resolves to the Codex CLI User-Agent so a gateway that
    // whitelists that client wire image accepts the request.
    let codex = capture_custom_provider_request(json!({
        "name": "AgentRouter",
        "base_url": "http://unused/v1",
        "models": ["gpt-5.6-sol"],
        "requires_auth": false,
        "client_fingerprint": "codex"
    }))
    .await
    .to_ascii_lowercase();
    assert!(
        codex.contains("user-agent: codex_cli_rs/0.144.1 (mac os; arm64)"),
        "codex fingerprint UA missing: {codex}"
    );

    // A raw user_agent overrides the named preset entirely.
    let raw = capture_custom_provider_request(json!({
        "name": "AgentRouter",
        "base_url": "http://unused/v1",
        "models": ["gpt-5.6-sol"],
        "requires_auth": false,
        "client_fingerprint": "codex",
        "user_agent": "claude-cli/9.9.9 (external, cli)"
    }))
    .await
    .to_ascii_lowercase();
    assert!(
        raw.contains("user-agent: claude-cli/9.9.9 (external, cli)"),
        "raw user_agent override missing: {raw}"
    );
    assert!(
        !raw.contains("codex_cli_rs"),
        "raw override must replace the preset UA: {raw}"
    );

    // Extra headers ride along verbatim for gateways that gate on more than UA.
    let headers = capture_custom_provider_request(json!({
        "name": "AgentRouter",
        "base_url": "http://unused/v1",
        "models": ["gpt-5.6-sol"],
        "requires_auth": false,
        "headers": { "X-Client-Id": "codex-desktop" }
    }))
    .await
    .to_ascii_lowercase();
    assert!(
        headers.contains("x-client-id: codex-desktop"),
        "custom header missing: {headers}"
    );
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn openai_compat_committed_stream_propagates_instead_of_restarting() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 1024];
        let _ = conn.read(&mut scratch).await;
        conn.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        conn.write_all(b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")
            .await
            .unwrap();
        conn.flush().await.unwrap();
        // Hold the connection open far past the idle window: the test asserts
        // that the IDLE TIMEOUT ends the stream, and a 600 ms hold raced it —
        // under load the client read "partial" late, the server had already
        // hung up, and EOF arrived where the timeout was expected. The task is
        // aborted at the end of the test.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });

    let _idle = EnvVarGuard::set(crate::providers::STREAM_IDLE_TIMEOUT_ENV, Some("300"));
    let client = OpenAiCompatClient::new("token", OpenAiCompatConfig::openai())
        .with_base_url(format!("http://{addr}/v1"))
        .with_retry_policy(
            3,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );
    let mut stream = client
        .stream_message(&streaming_request())
        .await
        .expect("open stream");

    let mut saw_partial = false;
    while let Some(event) = stream.next_event().await.expect("events before stall ok") {
        if let StreamEvent::ContentBlockDelta(delta) = event {
            if let ContentBlockDelta::TextDelta { text } = delta.delta {
                assert_eq!(text, "partial");
                saw_partial = true;
                break;
            }
        }
    }
    assert!(saw_partial, "stream must surface the committing text delta");
    let error = stream.next_event().await.expect_err("post-commit error");
    assert!(
        matches!(
            error,
            ApiError::StreamApi { error_type, .. }
                if error_type.as_deref() == Some("stream_idle_timeout")
        ),
        "post-commit idle must propagate"
    );
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    server.abort();
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn openai_compat_restart_reopen_is_bounded_by_the_wallclock_budget() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_hits = hits.clone();

    let server = tokio::spawn(async move {
        // First stream opens and goes silent pre-commit.
        let (mut first, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut scratch = [0u8; 1024];
        let _ = first.read(&mut scratch).await;
        first
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        first.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(450)).await;

        // Reopen: accept the connection but never answer it. Without the
        // remaining-window timeout around the reopen this would park the
        // stream indefinitely (the shared client has no blanket timeout).
        let (mut second, _) = listener.accept().await.unwrap();
        server_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = second.read(&mut scratch).await;
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    });

    let _idle = EnvVarGuard::set(crate::providers::STREAM_IDLE_TIMEOUT_ENV, Some("300"));
    let client = OpenAiCompatClient::new("token", OpenAiCompatConfig::openai())
        .with_base_url(format!("http://{addr}/v1"))
        .with_retry_policy(
            3,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );
    let mut stream = client
        .stream_message(&streaming_request())
        .await
        .expect("open stream");
    // Shrink the restart window so the never-answered reopen fails out in
    // test time instead of the production 120s ceiling.
    stream.max_restart_wallclock = std::time::Duration::from_millis(500);

    let started = std::time::Instant::now();
    let error = stream.next_event().await.expect_err("reopen must time out");
    assert!(
        matches!(
            &error,
            ApiError::StreamApi { error_type, .. }
                if error_type.as_deref() == Some("stream_restart_timeout")
        ),
        "expected stream_restart_timeout, got: {error:?}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "reopen wait must be bounded by the restart window, took {:?}",
        started.elapsed()
    );
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    server.abort();
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn openai_compat_restart_fires_the_retry_notice_sink() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let _guard = env_lock();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // First stream opens and goes silent pre-commit.
        let (mut first, _) = listener.accept().await.unwrap();
        let mut scratch = [0u8; 1024];
        let _ = first.read(&mut scratch).await;
        first
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        first.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(450)).await;

        // Second stream recovers.
        let (mut second, _) = listener.accept().await.unwrap();
        let _ = second.read(&mut scratch).await;
        let body = concat!(
            "data: {\"id\":\"c2\",\"choices\":[{\"delta\":{\"content\":\"recovered\"}}]}\n\n",
            "data: {\"id\":\"c2\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],",
            "\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n"
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        second.write_all(head.as_bytes()).await.unwrap();
        second.write_all(body.as_bytes()).await.unwrap();
        second.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    });

    let _idle = EnvVarGuard::set(crate::providers::STREAM_IDLE_TIMEOUT_ENV, Some("300"));
    let client = OpenAiCompatClient::new("token", OpenAiCompatConfig::openai())
        .with_base_url(format!("http://{addr}/v1"))
        .with_retry_policy(
            3,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        );
    let notices = std::sync::Arc::new(Mutex::new(Vec::new()));
    let sink = notices.clone();
    let mut stream = client
        .stream_message(&streaming_request())
        .await
        .expect("open stream")
        .with_retry_notice_callback(move |notice| sink.lock().unwrap().push(notice));

    let mut text = String::new();
    while let Some(event) = stream.next_event().await.expect("restart should recover") {
        if let StreamEvent::ContentBlockDelta(delta) = &event {
            if let ContentBlockDelta::TextDelta { text: chunk } = &delta.delta {
                text.push_str(chunk);
            }
        }
    }
    server.await.unwrap();
    assert_eq!(text, "recovered");

    let notices = notices.lock().unwrap();
    assert_eq!(
        notices.len(),
        1,
        "exactly one restart must fire exactly one notice"
    );
    assert_eq!(notices[0].attempt, 1);
    assert_eq!(notices[0].max_attempts, 3);
    assert_eq!(
        notices[0].label, "transient provider error",
        "idle timeout classifies as a transient (non-capacity) fault"
    );
}

/// Anthropic reasoning is provider-opaque and must never reach an OpenAI-
/// compatible request: the encoder drops both `thinking` and `redacted_thinking`
/// while keeping the assistant's visible text and tool call.
#[test]
fn thinking_blocks_are_dropped_from_openai_request() {
    let payload = build_chat_completion_request(
        &MessageRequest {
            model: "grok-3".to_string(),
            max_tokens: 64,
            messages: vec![InputMessage {
                role: "assistant".to_string(),
                content: vec![
                    InputContentBlock::Thinking {
                        thinking: "SECRETREASONING_XYZ".to_string(),
                        signature: "THINKSIG_XYZ".to_string(),
                    },
                    InputContentBlock::RedactedThinking {
                        data: "REDACTEDBLOB_XYZ".to_string(),
                    },
                    InputContentBlock::Text {
                        text: "the answer".to_string(),
                        cache_control: None,
                    },
                    InputContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "weather".to_string(),
                        input: json!({"city": "SF"}),
                                            cache_control: None,
                    },
                ],
                thought_signature: None,
                reasoning_replay: None,
            }],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        },
        OpenAiCompatConfig::xai(),
    );

    let serialized = serde_json::to_string(&payload).expect("serialize");
    assert!(
        !serialized.contains("SECRETREASONING_XYZ"),
        "reasoning text leaked to OpenAI: {serialized}"
    );
    assert!(
        !serialized.contains("THINKSIG_XYZ"),
        "thinking signature leaked to OpenAI: {serialized}"
    );
    assert!(
        !serialized.contains("REDACTEDBLOB_XYZ"),
        "redacted thinking leaked to OpenAI: {serialized}"
    );
    // The visible content survives the drop.
    assert!(serialized.contains("the answer"), "{serialized}");
    assert!(serialized.contains("weather"), "{serialized}");
}
