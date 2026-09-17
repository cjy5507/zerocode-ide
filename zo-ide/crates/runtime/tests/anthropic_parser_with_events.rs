//! L7c-1b — seam test for `AnthropicStream::parse_source_with_events`.
//!
//! Verifies that the new parser variant produces:
//!
//! 1. A `RenderBlock` stream **byte-equivalent** to the existing
//!    `parse_source` path (the TUI feel is unchanged).
//! 2. An `AssistantEvent` sequence matching what the legacy
//!    `zo_cli::main::AnthropicRuntimeClient::stream`
//!    collector builds from the same SSE frames, so that
//!    `ConversationRuntime::run_turn_streaming`'s bookkeeping path
//!    (`build_assistant_message`) remains byte-equivalent when L7c-2
//!    swaps it onto this helper.
//!
//! This is the guardrail Lead attached to the L7c-1b sub-commit in
//! `.zo/tasks/L7c-tui-integration.md`.

use api::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    MessageDelta, MessageDeltaEvent, MessageResponse, MessageStartEvent, MessageStopEvent,
    OutputContentBlock, StreamEvent, Usage,
};
use runtime::message_stream::anthropic::AnthropicStream;
use runtime::message_stream::anthropic::source::VecSource;
use runtime::message_stream::{BlockIdGen, RenderBlock};
use runtime::{AssistantEvent, OutputTokensDetails, TokenUsage};
use serde_json::json;
use tokio::sync::mpsc;

fn message_start() -> StreamEvent {
    StreamEvent::MessageStart(MessageStartEvent {
        message: MessageResponse {
            id: "msg_test".into(),
            kind: "message".into(),
            role: "assistant".into(),
            content: vec![],
            model: "claude-test".into(),
            stop_reason: None,
            stop_sequence: None,
            usage: Usage {
                input_tokens: 11,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens: 0,
                output_tokens_details: None,
            },
            request_id: None,
            thought_signature: None,
            reasoning_replay: None,
            context_management: None,
        },
    })
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

fn block_stop(index: u32) -> StreamEvent {
    StreamEvent::ContentBlockStop(ContentBlockStopEvent { index })
}

fn message_delta(output_tokens: u32) -> StreamEvent {
    StreamEvent::MessageDelta(MessageDeltaEvent {
        delta: MessageDelta {
            stop_reason: Some("end_turn".into()),
            stop_sequence: None,
            thought_signature: None,
            reasoning_replay: None,
        },
        usage: Usage {
            input_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            output_tokens,
            output_tokens_details: None,
        },
        context_management: None,
    })
}

fn tool_use_start(index: u32, id: &str, name: &str) -> StreamEvent {
    StreamEvent::ContentBlockStart(ContentBlockStartEvent {
        index,
        content_block: OutputContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input: json!({}),
        },
    })
}

fn input_json_delta(index: u32, partial_json: &str) -> StreamEvent {
    StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
        index,
        delta: ContentBlockDelta::InputJsonDelta {
            partial_json: partial_json.into(),
        },
    })
}

/// Drive the L7c-1b helper once and collect both streams.
async fn drive(events: Vec<StreamEvent>) -> (Vec<RenderBlock>, Vec<AssistantEvent>) {
    let (tx, mut rx) = mpsc::channel::<RenderBlock>(64);
    let ids = BlockIdGen::default();
    let collector = tokio::spawn(async move {
        let mut out = Vec::new();
        while let Some(block) = rx.recv().await {
            out.push(block);
        }
        out
    });
    let outputs = AnthropicStream::parse_source_with_events(VecSource::new(events), tx, ids)
        .await
        .expect("parse_source_with_events");
    let blocks = collector.await.expect("drain render blocks");
    (blocks, outputs.events)
}

/// Mirror collector driven through `parse_source` (RenderBlock-only).
/// Used to assert the `RenderBlock` stream emitted by
/// `parse_source_with_events` is byte-equivalent to the L7b path.
async fn drive_render_only(events: Vec<StreamEvent>) -> Vec<RenderBlock> {
    let (tx, mut rx) = mpsc::channel::<RenderBlock>(64);
    let ids = BlockIdGen::default();
    let collector = tokio::spawn(async move {
        let mut out = Vec::new();
        while let Some(block) = rx.recv().await {
            out.push(block);
        }
        out
    });
    AnthropicStream::parse_source(VecSource::new(events), tx, ids)
        .await
        .expect("parse_source");
    collector.await.expect("drain render blocks")
}

/// A non-streaming response folded into a synthetic `message_start`: the whole
/// turn's content arrives already-complete on the start frame (the Gemini and
/// ChatGPT backends, and any provider's non-stream fallback, do this).
fn message_start_with_content(content: Vec<OutputContentBlock>) -> StreamEvent {
    StreamEvent::MessageStart(MessageStartEvent {
        message: MessageResponse {
            id: "msg_test".into(),
            kind: "message".into(),
            role: "assistant".into(),
            content,
            model: "gemini-test".into(),
            stop_reason: None,
            stop_sequence: None,
            usage: Usage {
                input_tokens: 7,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens: 0,
                output_tokens_details: None,
            },
            request_id: None,
            thought_signature: None,
            reasoning_replay: None,
            context_management: None,
        },
    })
}

#[tokio::test]
async fn message_start_content_blocks_render_for_non_streaming_providers() {
    // Regression: the render path used to read only `input_tokens` from
    // `message_start` and drop its content, so a provider that folds the whole
    // turn into a synthetic `message_start` (Gemini/ChatGPT, or any non-stream
    // fallback) rendered nothing — the user saw no diff/markdown despite the
    // assistant having produced it. The blocks are complete, so they must
    // render as start→finish pairs just like a streamed turn.
    let (blocks, events) = drive(vec![
        message_start_with_content(vec![
            OutputContentBlock::Text {
                text: "Here is the patch.".into(),
            },
            OutputContentBlock::ToolUse {
                id: "call_1".into(),
                name: "edit_file".into(),
                input: json!({ "path": "src/main.rs" }),
            },
        ]),
        message_delta(5),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ])
    .await;

    let rendered_text = blocks.iter().any(|block| {
        matches!(block, RenderBlock::TextDelta { text, .. } if text.contains("Here is the patch."))
    });
    assert!(
        rendered_text,
        "message_start text content must render to the TUI, not be dropped: {blocks:#?}"
    );
    let rendered_tool_call = blocks
        .iter()
        .any(|block| matches!(block, RenderBlock::ToolCall { name, .. } if name == "edit_file"));
    assert!(
        rendered_tool_call,
        "message_start tool_use content must render to the TUI: {blocks:#?}"
    );

    // The AssistantEvent bookkeeping path already surfaced these; the render
    // path now matches it, so both views agree on the same turn.
    assert!(
        events.iter().any(
            |event| matches!(event, AssistantEvent::ToolUse { name, .. } if name == "edit_file")
        ),
        "event path still reports the tool call so render/event stay in lockstep: {events:#?}"
    );
}

#[tokio::test]
async fn message_delta_thought_signature_emits_provider_state_event() {
    let (_blocks, events) = drive(vec![
        message_start(),
        text_start(0),
        text_delta(0, "ok"),
        block_stop(0),
        StreamEvent::MessageDelta(MessageDeltaEvent {
            delta: MessageDelta {
                stop_reason: Some("end_turn".into()),
                stop_sequence: None,
                thought_signature: Some("SIG_STREAM".into()),
                reasoning_replay: None,
            },
            usage: Usage {
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens: 1,
                output_tokens_details: None,
            },
            context_management: None,
        }),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ])
    .await;

    assert!(events.iter().any(|event| matches!(
        event,
        AssistantEvent::ProviderState(state)
            if state.as_gemini_thought_signature() == Some("SIG_STREAM")
    )));
}

#[tokio::test]
async fn render_block_stream_is_byte_equivalent_to_parse_source() {
    let scenario = vec![
        message_start(),
        text_start(0),
        text_delta(0, "Hello, "),
        text_delta(0, "world!"),
        block_stop(0),
        message_delta(5),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ];

    let legacy_blocks = drive_render_only(scenario.clone()).await;
    let (new_blocks, _events) = drive(scenario).await;

    // `parse_source_with_events` additionally emits a live `RenderBlock::Usage`
    // ctx snapshot at `message_start` — it updates the HUD ledger, never the
    // transcript (see the type's doc comment). Filter those out: the
    // *transcript* render stream must stay byte-identical to the L7b path.
    let new_transcript_blocks: Vec<_> = new_blocks
        .into_iter()
        .filter(|block| !matches!(block, RenderBlock::Usage { .. }))
        .collect();

    // `RenderBlock` does not derive `PartialEq` (it carries
    // provider-opaque payload variants); compare via the `Debug`
    // projection, which is structurally exhaustive and stable.
    assert_eq!(
        format!("{legacy_blocks:#?}"),
        format!("{new_transcript_blocks:#?}"),
        "parse_source_with_events must emit a byte-equivalent transcript RenderBlock stream"
    );
}

#[tokio::test]
async fn emits_live_ctx_usage_snapshot_at_message_start() {
    // The live ctx fix: the input side of the turn (here `input_tokens: 11`
    // from `message_start`) must surface as a `RenderBlock::Usage` *before*
    // any text renders, so the HUD ledger tracks window occupancy the moment
    // the request lands instead of snapping only when the turn closes.
    let (blocks, _events) = drive(vec![
        message_start(),
        text_start(0),
        text_delta(0, "Hi"),
        block_stop(0),
        message_delta(5),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ])
    .await;

    let usage_pos = blocks
        .iter()
        .position(|block| matches!(block, RenderBlock::Usage { .. }))
        .expect("a live ctx Usage snapshot should be emitted at message_start");
    match &blocks[usage_pos] {
        RenderBlock::Usage {
            ctx_tokens,
            cumulative,
            current,
        } => {
            assert_eq!(*ctx_tokens, 11, "ctx preview = message_start input side");
            assert_eq!(
                cumulative.total_tokens(),
                0,
                "a mid-stream ctx snapshot carries no cumulative (cost not yet known)"
            );
            assert_eq!(
                current.total_tokens(),
                0,
                "a mid-stream ctx snapshot carries no current-turn split yet"
            );
        }
        _ => unreachable!("filtered to Usage above"),
    }

    // The ctx snapshot must lead the text it accompanies.
    if let Some(text_pos) = blocks
        .iter()
        .position(|block| matches!(block, RenderBlock::TextDelta { .. }))
    {
        assert!(
            usage_pos < text_pos,
            "the live ctx snapshot must precede the text stream"
        );
    }
}

#[tokio::test]
async fn assistant_events_text_only_turn_matches_legacy_collector() {
    let (_blocks, events) = drive(vec![
        message_start(),
        text_start(0),
        text_delta(0, "Hello, "),
        text_delta(0, "world!"),
        block_stop(0),
        message_delta(5),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ])
    .await;

    assert_eq!(
        events,
        vec![
            // `message_start` names the serving model; it leads every event
            // list for per-model cost attribution.
            AssistantEvent::Model("claude-test".to_string()),
            AssistantEvent::TextDelta("Hello, ".to_string()),
            AssistantEvent::TextDelta("world!".to_string()),
            AssistantEvent::Usage(TokenUsage {
                input_tokens: 11,
                output_tokens: 5,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens_details: None,
            }),
            AssistantEvent::MessageStop,
            // The provider's stop reason is surfaced as a trailing event so the
            // conversation loop can tell a natural end from output-limit
            // truncation. `message_delta` reports `end_turn` here.
            AssistantEvent::StopReason("end_turn".to_string()),
        ]
    );
}

#[tokio::test]
async fn assistant_events_preserve_anthropic_output_tokens_details() {
    let details = OutputTokensDetails {
        thinking_tokens: 312,
    };
    let (_blocks, events) = drive(vec![
        message_start(),
        text_start(0),
        text_delta(0, "answer"),
        block_stop(0),
        StreamEvent::MessageDelta(MessageDeltaEvent {
            delta: MessageDelta {
                stop_reason: Some("end_turn".into()),
                stop_sequence: None,
                thought_signature: None,
                reasoning_replay: None,
            },
            usage: Usage {
                input_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens: 348,
                output_tokens_details: Some(details),
            },
            context_management: None,
        }),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ])
    .await;

    let usage = events
        .iter()
        .find_map(|event| match event {
            AssistantEvent::Usage(usage) => Some(*usage),
            _ => None,
        })
        .expect("usage event");
    assert_eq!(usage.output_tokens_details, Some(details));
}

#[tokio::test]
async fn assistant_events_tool_use_accumulates_input_json_delta() {
    let (_blocks, events) = drive(vec![
        message_start(),
        tool_use_start(0, "toolu_01", "bash"),
        input_json_delta(0, "{\"command\":\"ech"),
        input_json_delta(0, "o hi\"}"),
        block_stop(0),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ])
    .await;

    // The collector should emit exactly one ToolUse with the full
    // concatenated JSON, then MessageStop. Mirrors main.rs:~2041..2062.
    assert_eq!(
        events,
        vec![
            AssistantEvent::Model("claude-test".to_string()),
            AssistantEvent::ToolUse {
                id: "toolu_01".into(),
                name: "bash".into(),
                input: "{\"command\":\"echo hi\"}".into(),
            },
            AssistantEvent::MessageStop,
        ]
    );
}

#[tokio::test]
async fn assistant_events_flush_pending_tool_use_when_stream_ends_without_stops() {
    let (_blocks, events) = drive(vec![
        message_start(),
        tool_use_start(0, "toolu_01", "read"),
        input_json_delta(0, "{\"path\":\"/tmp/not"),
        input_json_delta(0, "es.md\"}"),
    ])
    .await;

    assert_eq!(
        events,
        vec![
            AssistantEvent::Model("claude-test".to_string()),
            AssistantEvent::ToolUse {
                id: "toolu_01".into(),
                name: "read".into(),
                input: "{\"path\":\"/tmp/notes.md\"}".into(),
            }
        ]
    );
}

#[tokio::test]
async fn assistant_events_capture_thinking_text_and_signature() {
    // CC-parity bug ④: the streaming path now accumulates ThinkingDelta /
    // SignatureDelta (seeded by the content_block_start placeholder) and flushes
    // one AssistantEvent::Thinking on the block stop, so the reasoning is stored
    // and can be replayed verbatim on the next Anthropic request. (It still
    // reaches the TUI via RenderBlock::Reasoning on the parallel render pass.)
    let (_blocks, events) = drive(vec![
        message_start(),
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 0,
            content_block: OutputContentBlock::Thinking {
                thinking: "planning".into(),
                signature: None,
            },
        }),
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 0,
            delta: ContentBlockDelta::ThinkingDelta {
                thinking: " more".into(),
            },
        }),
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 0,
            delta: ContentBlockDelta::SignatureDelta {
                signature: "sig".into(),
            },
        }),
        block_stop(0),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ])
    .await;

    assert_eq!(
        events,
        vec![
            AssistantEvent::Model("claude-test".to_string()),
            AssistantEvent::Thinking {
                thinking: "planning more".into(),
                signature: Some("sig".into()),
            },
            AssistantEvent::MessageStop,
        ]
    );
}
