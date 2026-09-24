//! Snapshot tests for the Anthropic `message_stream` adapter.
//!
//! Each test feeds a canned sequence of `api::StreamEvent`s into the
//! adapter and asserts the resulting provider-neutral `RenderBlock`
//! sequence. The tests cover:
//!
//! * Plain text deltas (including multi-chunk streaming).
//! * `tool_use` with `input_json_delta` accumulation.
//! * `thinking_delta` + `signature_delta` preservation (R6).
//! * `message_delta` finalisation with token usage.
//! * Tool formatter outputs for all 15 canonical tool names via the
//!   pure `format_tool_result` helper (batched as several tests).
//!
//! Living standard: `mod_<scenario>` test-naming, snapshot style uses
//! exact structural assertions (no `insta` here because L1 keeps the
//! dep surface tight).

use api::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    MessageDelta, MessageDeltaEvent, MessageResponse, MessageStartEvent, MessageStopEvent,
    OutputContentBlock, StreamEvent, Usage,
};
use runtime::message_stream::{
    anthropic::{
        parser::render_tool_result,
        tools::{
            format_bash_result, format_edit_result, format_generic_result, format_glob_result,
            format_grep_result, format_read_result, format_tool_result, format_write_result,
            preview_tool_input,
        },
        AnthropicStream,
    },
    BashResult, BlockIdGen, DiffLineKind, RenderBlock, ToolCallStatus, ToolPreview, ToolResultBody,
};
use serde_json::json;
use tokio::sync::mpsc;

/// Collect every `RenderBlock` emitted by the adapter for a given
/// canned event sequence.
async fn drive(
    events: Vec<StreamEvent>,
) -> (Vec<RenderBlock>, runtime::message_stream::TurnSummary) {
    let (tx, mut rx) = mpsc::channel::<RenderBlock>(64);
    let ids = BlockIdGen::default();
    let collector = tokio::spawn(async move {
        let mut out = Vec::new();
        while let Some(block) = rx.recv().await {
            out.push(block);
        }
        out
    });
    let summary = AnthropicStream::parse(events, tx, ids).await.unwrap();
    let blocks = collector.await.unwrap();
    (blocks, summary)
}

fn fake_message_start() -> StreamEvent {
    StreamEvent::MessageStart(MessageStartEvent {
        message: MessageResponse {
            id: "msg_test".into(),
            kind: "message".into(),
            role: "assistant".into(),
            content: vec![],
            model: "claude-test".into(),
            stop_reason: None,
            stop_sequence: None,
            stop_details: None,
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

fn text_start(index: u32, text: &str) -> StreamEvent {
    StreamEvent::ContentBlockStart(ContentBlockStartEvent {
        index,
        content_block: OutputContentBlock::Text {
            text: text.to_string(),
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

fn message_delta(stop_reason: &str, output_tokens: u32) -> StreamEvent {
    StreamEvent::MessageDelta(MessageDeltaEvent {
        delta: MessageDelta {
            stop_reason: Some(stop_reason.to_string()),
            stop_sequence: None,
            stop_details: None,
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

#[tokio::test]
async fn mod_text_delta_streaming_concatenates_chunks() {
    let events = vec![
        fake_message_start(),
        text_start(0, ""),
        text_delta(0, "Hello, "),
        text_delta(0, "world!"),
        block_stop(0),
        message_delta("end_turn", 5),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ];
    let (blocks, summary) = drive(events).await;

    let texts: Vec<_> = blocks
        .iter()
        .filter_map(|b| match b {
            RenderBlock::TextDelta { text, done, .. } => Some((text.clone(), *done)),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            ("Hello, ".into(), false),
            ("world!".into(), false),
            (String::new(), true),
        ]
    );
    assert_eq!(summary.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(summary.output_tokens, 5);
    assert_eq!(summary.input_tokens, 7);
}

#[tokio::test]
async fn mod_tool_use_accumulates_input_json_delta() {
    let events = vec![
        fake_message_start(),
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 0,
            content_block: OutputContentBlock::ToolUse {
                id: "toolu_01".into(),
                name: "bash".into(),
                input: json!({}),
            },
        }),
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 0,
            delta: ContentBlockDelta::InputJsonDelta {
                partial_json: "{\"command\":\"ech".into(),
            },
        }),
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 0,
            delta: ContentBlockDelta::InputJsonDelta {
                partial_json: "o hi\"}".into(),
            },
        }),
        block_stop(0),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ];
    let (blocks, _) = drive(events).await;

    // Expect: pending ToolCall (from start) + running ToolCall (from stop).
    let calls: Vec<_> = blocks
        .iter()
        .filter_map(|b| match b {
            RenderBlock::ToolCall {
                status, preview, ..
            } => Some((*status, preview.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, ToolCallStatus::Pending);
    assert_eq!(calls[1].0, ToolCallStatus::Running);
    match &calls[1].1 {
        ToolPreview::Bash { command } => assert_eq!(command, "echo hi"),
        other => panic!("expected bash preview, got {other:?}"),
    }
}

#[tokio::test]
async fn mod_thinking_delta_surfaces_reasoning() {
    let events = vec![
        fake_message_start(),
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 0,
            content_block: OutputContentBlock::Thinking {
                thinking: String::new(),
                signature: None,
            },
        }),
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 0,
            delta: ContentBlockDelta::ThinkingDelta {
                thinking: "Let me think…".into(),
            },
        }),
        StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
            index: 0,
            delta: ContentBlockDelta::SignatureDelta {
                signature: "sigABC".into(),
            },
        }),
        block_stop(0),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ];
    let (blocks, _) = drive(events).await;

    let reasoning: Vec<_> = blocks
        .iter()
        .filter_map(|b| match b {
            RenderBlock::Reasoning {
                text,
                signature,
                done,
                ..
            } => Some((text.clone(), signature.clone(), *done)),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning.len(), 2);
    assert_eq!(reasoning[0].0, "Let me think…");
    assert!(!reasoning[0].2);
    // Final emission should carry signature and done = true.
    assert!(reasoning[1].2);
    assert_eq!(reasoning[1].1.as_deref(), Some("sigABC"));
}

#[tokio::test]
async fn mod_text_delta_without_start_is_recovered() {
    // Regression guard for R6 — deltas arriving before a start must
    // not be silently dropped.
    let events = vec![
        fake_message_start(),
        text_delta(0, "orphan"),
        block_stop(0),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ];
    let (blocks, _) = drive(events).await;
    assert!(blocks
        .iter()
        .any(|b| matches!(b, RenderBlock::TextDelta { text, .. } if text == "orphan")));
}

#[tokio::test]
async fn mod_message_delta_updates_summary_usage() {
    let events = vec![
        fake_message_start(),
        message_delta("max_tokens", 42),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ];
    let (_, summary) = drive(events).await;
    assert_eq!(summary.stop_reason.as_deref(), Some("max_tokens"));
    assert_eq!(summary.output_tokens, 42);
}

#[tokio::test]
async fn mod_redacted_thinking_emits_system_notice() {
    let events = vec![
        fake_message_start(),
        StreamEvent::ContentBlockStart(ContentBlockStartEvent {
            index: 0,
            content_block: OutputContentBlock::RedactedThinking {
                data: json!("opaque"),
            },
        }),
        block_stop(0),
        StreamEvent::MessageStop(MessageStopEvent {}),
    ];
    let (blocks, _) = drive(events).await;
    assert!(blocks
        .iter()
        .any(|b| matches!(b, RenderBlock::System { text, .. } if text.contains("redacted"))));
}

#[tokio::test]
async fn tools_bash_result_splits_streams() {
    let value = json!({
        "exit_code": 1,
        "stdout": "out line",
        "stderr": "boom",
    });
    let result: BashResult = format_bash_result(&value);
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.stdout, "out line");
    assert_eq!(result.stderr, "boom");
    assert!(!result.truncated);
}

#[tokio::test]
async fn tools_read_result_detects_language() {
    let value = json!({
        "file": {
            "file_path": "src/lib.rs",
            "content": "fn main() {}",
            "startLine": 1,
            "numLines": 1,
            "totalLines": 1,
        }
    });
    match format_read_result(&value) {
        ToolResultBody::Read { language, path, .. } => {
            assert_eq!(language.as_deref(), Some("rust"));
            assert_eq!(path, "src/lib.rs");
        }
        other => panic!("expected Read body, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_edit_result_structured_patch_parses_hunks() {
    let value = json!({
        "file_path": "a.rs",
        "structuredPatch": [
            {
                "oldStart": 10, "oldLines": 2,
                "newStart": 10, "newLines": 3,
                "lines": [" context", "-old", "+new1", "+new2"]
            }
        ]
    });
    match format_edit_result(&value) {
        ToolResultBody::Diff(view) => {
            assert_eq!(view.hunks.len(), 1);
            let hunk = &view.hunks[0];
            assert_eq!(hunk.old_start, 10);
            assert_eq!(hunk.new_lines, 3);
            let kinds: Vec<_> = hunk.lines.iter().map(|l| l.kind).collect();
            assert_eq!(
                kinds,
                vec![
                    DiffLineKind::Context,
                    DiffLineKind::Removed,
                    DiffLineKind::Added,
                    DiffLineKind::Added,
                ]
            );
        }
        other => panic!("expected diff, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_edit_preview_never_displays_raw_partial_payload() {
    let raw = serde_json::Value::String(
        r#"{"file_path":"src/lib.rs","old_string":"very long escaped source...""#.to_string(),
    );

    match preview_tool_input("edit_file", &raw) {
        ToolPreview::Edit { path, hunk_count } => {
            assert_eq!(path, "?");
            assert_eq!(hunk_count, 1);
        }
        other => panic!("expected edit preview, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_edit_result_accepts_snake_case_structured_patch() {
    let value = json!({
        "file_path": "a.rs",
        "structured_patch": [
            {
                "oldStart": 1, "oldLines": 1,
                "newStart": 1, "newLines": 1,
                "lines": ["-old", "+new"]
            }
        ]
    });
    match format_edit_result(&value) {
        ToolResultBody::Diff(view) => {
            assert_eq!(view.hunks.len(), 1);
            assert_eq!(view.hunks[0].lines.len(), 2);
        }
        other => panic!("expected diff, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_edit_result_fallback_replace_strings() {
    let value = json!({
        "file_path": "a.rs",
        "oldString": "foo",
        "newString": "bar",
    });
    match format_edit_result(&value) {
        ToolResultBody::Diff(view) => {
            assert_eq!(view.hunks.len(), 1);
            assert_eq!(view.hunks[0].lines.len(), 2);
        }
        other => panic!("expected diff, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_edit_result_fallback_replace_strings_compacts_common_context() {
    let old = (1..=12)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut new_lines = (1..=12).map(|n| format!("line {n}")).collect::<Vec<_>>();
    new_lines[7] = "changed".to_string();
    let new = new_lines.join("\n");
    let value = json!({
        "file_path": "a.rs",
        "oldString": old,
        "newString": new,
    });

    match format_edit_result(&value) {
        ToolResultBody::Diff(view) => {
            let hunk = &view.hunks[0];
            assert_eq!(hunk.old_start, 5);
            let rendered = hunk
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>();
            assert!(rendered.contains(&"line 8"));
            assert!(rendered.contains(&"changed"));
            assert!(
                !rendered.contains(&"line 1"),
                "distant unchanged prefix should stay hidden: {rendered:?}"
            );
            assert!(
                !rendered.contains(&"line 12"),
                "distant unchanged suffix should stay hidden: {rendered:?}"
            );
        }
        other => panic!("expected diff, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_edit_result_fallback_replace_strings_splits_distant_hunks() {
    let old = (1..=30)
        .map(|n| {
            if n == 5 || n == 20 {
                format!("needle {n}")
            } else {
                format!("line {n}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let new = old.replace("needle", "changed");
    let value = json!({
        "file_path": "a.rs",
        "oldString": old,
        "newString": new,
    });

    match format_edit_result(&value) {
        ToolResultBody::Diff(view) => {
            assert_eq!(view.hunks.len(), 2);
            let rendered = view
                .hunks
                .iter()
                .flat_map(|hunk| hunk.lines.iter())
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>();
            assert!(rendered.contains(&"needle 5"));
            assert!(rendered.contains(&"changed 5"));
            assert!(rendered.contains(&"needle 20"));
            assert!(rendered.contains(&"changed 20"));
            assert!(
                !rendered.contains(&"line 12"),
                "unchanged lines between distant edits should stay hidden: {rendered:?}"
            );
        }
        other => panic!("expected diff, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_write_result_summary() {
    let value = json!({
        "file_path": "new.md",
        "type": "create",
        "content": "a\nb\nc\n",
    });
    match format_write_result("write_file", &value) {
        ToolResultBody::Generic { content, .. } => {
            assert!(content.contains("Wrote"));
            assert!(content.contains("new.md"));
            assert!(content.contains("3 lines"));
        }
        other => panic!("expected generic body, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_glob_result_lists_filenames() {
    let value = json!({
        "numFiles": 2,
        "filenames": ["a.rs", "b.rs"],
    });
    match format_glob_result(&value) {
        ToolResultBody::Listing { entries, truncated } => {
            assert_eq!(entries, vec!["a.rs".to_string(), "b.rs".to_string()]);
            assert!(!truncated);
        }
        other => panic!("expected listing, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_grep_result_prefers_content_over_listing() {
    let value = json!({
        "content": "src/lib.rs:1:foo\n",
        "filenames": ["src/lib.rs"],
    });
    match format_grep_result(&value) {
        ToolResultBody::Text { content, .. } => assert!(content.contains("foo")),
        other => panic!("expected text body, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_generic_result_truncates_large_payloads() {
    let value = json!({"key": "v".repeat(50)});
    match format_generic_result("custom", &value) {
        ToolResultBody::Generic { name, .. } => assert_eq!(name, "custom"),
        other => panic!("expected generic, got {other:?}"),
    }
}

#[tokio::test]
async fn tools_preview_dispatch_covers_canonical_names() {
    assert!(matches!(
        preview_tool_input("bash", &json!({"command": "ls"})),
        ToolPreview::Bash { .. }
    ));
    assert!(matches!(
        preview_tool_input("read_file", &json!({"path": "a"})),
        ToolPreview::Read { .. }
    ));
    assert!(matches!(
        preview_tool_input("write_file", &json!({"path": "a", "content": "x"})),
        ToolPreview::Write { .. }
    ));
    assert!(matches!(
        preview_tool_input("edit_file", &json!({"path": "a"})),
        ToolPreview::Edit { .. }
    ));
    assert!(matches!(
        preview_tool_input("glob_search", &json!({"pattern": "*"})),
        ToolPreview::Glob { .. }
    ));
    assert!(matches!(
        preview_tool_input("grep_search", &json!({"pattern": "re"})),
        ToolPreview::Grep { .. }
    ));
    assert!(matches!(
        preview_tool_input("web_search", &json!({"query": "q"})),
        ToolPreview::Search { .. }
    ));
    assert!(matches!(
        preview_tool_input("mcp__foo__bar", &json!({"k": "v"})),
        ToolPreview::Generic { .. }
    ));
}

#[tokio::test]
async fn tools_format_tool_result_error_is_text() {
    let body = format_tool_result("bash", &json!("bad command"), true);
    match body {
        ToolResultBody::Text { content, .. } => assert_eq!(content, "bad command"),
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn parser_render_tool_result_routes_by_name() {
    let body = render_tool_result("bash", &json!({"exit_code": 0, "stdout": "ok"}), false);
    assert!(matches!(body, ToolResultBody::Bash(_)));
}
