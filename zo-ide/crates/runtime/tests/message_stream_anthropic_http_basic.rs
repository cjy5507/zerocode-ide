//! L7b — Anthropic HTTP SSE wiring tests.
//!
//! These tests exercise the production parser code path
//! ([`runtime::message_stream::anthropic::parser::parse_stream_async`])
//! by feeding fixture SSE bytes through the same `api::SseParser` the
//! live HTTP source uses, then handing the resulting events to an
//! [`EventSource`] implementation.
//!
//! No live network and no HTTP-mock dev-dep is required: the only
//! gap from production is `reqwest::Response::chunk()` itself, which
//! is already covered by the integration tests in `crates/api`.
//!
//! Coverage:
//!
//! 1. `text_only_stream_translates_to_text_render_blocks` — golden
//!    text-only fixture → expected `RenderBlock` sequence.
//! 2. `bash_tool_use_promotes_to_typed_preview` — `tool_use` SSE with
//!    `input_json_delta` chunks produces a `ToolPreview::Bash`, NOT
//!    `ToolPreview::Generic`.
//! 3. `unrecovered_disconnect_closes_open_blocks_before_surfacing_transport_error`
//!    — a partial prefix followed by a failure the reconnects cannot
//!    rescue closes the open blocks and surfaces `StreamError::Transport`.
//! 4. `dropped_receiver_aborts_promptly` — the parser stops as soon
//!    as the `RenderBlock` receiver is dropped, in bounded time, with
//!    no panic.

use std::collections::VecDeque;
use std::pin::Pin;

use api::{SseParser, StreamEvent};
use runtime::message_stream::provider::StreamError;
use runtime::message_stream::{
    anthropic::{
        source::{EventSource, VecSource},
        AnthropicStream,
    },
    BlockIdGen, RenderBlock, ToolCallStatus, ToolPreview,
};
use tokio::sync::mpsc;

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/anthropic_sse");

/// Decode a fixture file into a `Vec<StreamEvent>` using the same
/// `api::SseParser` the live HTTP path uses.
fn load_fixture(name: &str) -> Vec<StreamEvent> {
    let path = format!("{FIXTURE_DIR}/{name}");
    let bytes = std::fs::read(&path).unwrap_or_else(|err| {
        panic!("failed to read fixture {path}: {err}");
    });

    let mut parser = SseParser::new();
    let mut events = parser.push(&bytes).expect("fixture should parse");
    events.extend(parser.finish().expect("fixture trailer should parse"));
    events
}

async fn drain(rx: &mut mpsc::Receiver<RenderBlock>) -> Vec<RenderBlock> {
    let mut blocks = Vec::new();
    while let Some(block) = rx.recv().await {
        blocks.push(block);
    }
    blocks
}

#[tokio::test]
async fn text_only_stream_translates_to_text_render_blocks() {
    let events = load_fixture("text_only.sse");
    assert!(
        !events.is_empty(),
        "fixture should produce at least one event"
    );

    let (tx, mut rx) = mpsc::channel::<RenderBlock>(64);
    let ids = BlockIdGen::default();
    let task = tokio::spawn(async move {
        AnthropicStream::parse_source(VecSource::new(events), tx, ids)
            .await
            .expect("parse should succeed")
    });

    let blocks = drain(&mut rx).await;
    let summary = task.await.expect("join");
    assert_eq!(summary.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(summary.input_tokens, 12);
    assert_eq!(summary.output_tokens, 7);

    let text_chunks: Vec<&str> = blocks
        .iter()
        .filter_map(|b| match b {
            RenderBlock::TextDelta {
                text, done: false, ..
            } if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_chunks, vec!["Hello", " world"]);

    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, RenderBlock::TextDelta { done: true, .. })),
        "expected a final text delta with done=true; got: {blocks:?}"
    );
}

#[tokio::test]
async fn bash_tool_use_promotes_to_typed_preview() {
    let events = load_fixture("bash_tool_use.sse");

    let (tx, mut rx) = mpsc::channel::<RenderBlock>(64);
    let ids = BlockIdGen::default();
    let task = tokio::spawn(async move {
        AnthropicStream::parse_source(VecSource::new(events), tx, ids)
            .await
            .expect("parse should succeed")
    });

    let blocks = drain(&mut rx).await;
    let summary = task.await.expect("join");
    assert_eq!(summary.stop_reason.as_deref(), Some("tool_use"));

    let tool_calls: Vec<_> = blocks
        .iter()
        .filter_map(|b| match b {
            RenderBlock::ToolCall {
                preview,
                status,
                name,
                ..
            } => Some((name.clone(), preview.clone(), *status)),
            _ => None,
        })
        .collect();
    assert!(
        !tool_calls.is_empty(),
        "expected at least one ToolCall block"
    );

    // Last emission for the tool block should be Running with a typed
    // Bash preview reflecting the accumulated input_json_delta payload.
    let (name, preview, status) = tool_calls
        .last()
        .expect("at least one tool call block")
        .clone();
    assert_eq!(name, "bash");
    assert_eq!(status, ToolCallStatus::Running);
    match preview {
        ToolPreview::Bash { command } => assert_eq!(command, "ls -la"),
        other => panic!("expected ToolPreview::Bash, got {other:?}"),
    }
}

/// A stream that dies mid-turn and never comes back: it yields `prefix`, then
/// fails every later poll, so the parser's in-place reconnect budget is
/// exhausted instead of the source quietly recovering.
struct NeverRecoveringSource {
    prefix: VecDeque<StreamEvent>,
}

impl NeverRecoveringSource {
    fn new(prefix: Vec<StreamEvent>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

impl EventSource for NeverRecoveringSource {
    fn next_event<'a>(
        &'a mut self,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<Option<StreamEvent>, StreamError>> + Send + 'a>,
    > {
        Box::pin(async move {
            match self.prefix.pop_front() {
                Some(event) => Ok(Some(event)),
                None => Err(StreamError::Transport(
                    "simulated mid-stream disconnect".into(),
                )),
            }
        })
    }
}

/// A disconnect the reconnects cannot rescue still has to leave the TUI
/// consistent: every block open when the stream died gets its `done` sentinel
/// before the transport error propagates, so nothing renders as a live block
/// forever. Retry *counting* is unit-covered in `parser.rs`; this pins the
/// end-to-end contract on fixture-derived events.
///
/// Runs in real time: the reconnect backoff sleeps 1s + 3s before giving up.
#[tokio::test]
async fn unrecovered_disconnect_closes_open_blocks_before_surfacing_transport_error() {
    let prefix = load_fixture("text_only.sse")
        .into_iter()
        .take(3) // message_start, content_block_start, one delta
        .collect::<Vec<_>>();

    let (tx, mut rx) = mpsc::channel::<RenderBlock>(64);
    let ids = BlockIdGen::default();
    let task = tokio::spawn(async move {
        AnthropicStream::parse_source(NeverRecoveringSource::new(prefix), tx, ids).await
    });

    let blocks = drain(&mut rx).await;
    let result = task.await.expect("join");

    assert!(
        matches!(result, Err(StreamError::Transport(_))),
        "reconnects are bounded, so the disconnect must propagate; got {result:?}"
    );
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, RenderBlock::TextDelta { done: true, .. })),
        "the open text block must be closed with done=true; got: {blocks:?}"
    );
}

#[tokio::test]
async fn dropped_receiver_aborts_promptly() {
    // A pathologically long stream — many text deltas. We drop the
    // receiver after consuming a couple of them and assert the parser
    // exits with ChannelClosed in bounded time.
    let start_payload = serde_json::json!({
        "type": "message_start",
        "message": {
            "id": "msg_drop",
            "type": "message",
            "role": "assistant",
            "model": "claude-opus-4-6",
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": {
                "input_tokens": 0,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 0
            }
        }
    });
    let mut events: Vec<StreamEvent> =
        vec![serde_json::from_value(start_payload).expect("message_start payload")];
    events.push(StreamEvent::ContentBlockStart(
        api::ContentBlockStartEvent {
            index: 0,
            content_block: api::OutputContentBlock::Text {
                text: String::new(),
            },
        },
    ));
    for _ in 0..10_000 {
        events.push(StreamEvent::ContentBlockDelta(
            api::ContentBlockDeltaEvent {
                index: 0,
                delta: api::ContentBlockDelta::TextDelta {
                    text: "tick".to_string(),
                },
            },
        ));
    }

    let (tx, mut rx) = mpsc::channel::<RenderBlock>(2);
    let ids = BlockIdGen::default();
    let task = tokio::spawn(async move {
        AnthropicStream::parse_source(VecSource::new(events), tx, ids).await
    });

    // Pull a couple of blocks then drop the receiver.
    let _ = rx.recv().await;
    let _ = rx.recv().await;
    drop(rx);

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("parser should abort within 5s")
        .expect("join");
    match outcome {
        Err(StreamError::ChannelClosed) => {}
        other => panic!("expected ChannelClosed, got {other:?}"),
    }
}
