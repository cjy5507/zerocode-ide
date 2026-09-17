//! L7a — streaming conversation turn tests.
//!
//! Covers:
//! 1. `conversation_streaming_emits_same_logical_sequence_as_sync`
//! 2. `conversation_streaming_permission_prompt_round_trips`
//! 3. `conversation_streaming_cancels_when_receiver_dropped`
//!
//! The legacy sync `run_turn` tests live in
//! `runtime/src/conversation.rs` and are not touched by this file.

use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use runtime::message_stream::{RenderBlock, ToolCallStatus, ToolResultBody};
use runtime::permission::{
    ChannelPrompter, PermissionDecision as AsyncPermissionDecision, PermissionError,
    PermissionPrompter, PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::MessageRole;
use runtime::session::Session;
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ConcurrentDispatchFn, ContentBlock,
    ConversationMessage, ConversationRuntime, DEFAULT_STREAMING_CHANNEL_CAPACITY, PermissionMode,
    PermissionPolicy, RuntimeError, RuntimeFeatureConfig, RuntimeHookConfig, StaticToolExecutor,
    StreamingTurnError,
};
use tokio::sync::mpsc;

/// Scripted API client: one tool call on the first stream, plain text on
/// the second. Mirrors the canonical agent loop test fixture in
/// `conversation.rs`.
struct ScriptedApi {
    calls: usize,
}

impl ScriptedApi {
    fn new() -> Self {
        Self { calls: 0 }
    }
}

impl ApiClient for ScriptedApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.calls += 1;
        match self.calls {
            1 => Ok(vec![
                AssistantEvent::TextDelta("thinking".to_string()),
                AssistantEvent::ToolUse {
                    id: "tool-1".to_string(),
                    name: "echo".to_string(),
                    input: "hi".to_string(),
                },
                AssistantEvent::MessageStop,
            ]),
            _ => Ok(vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::MessageStop,
            ]),
        }
    }
}

/// Prompter that always returns `Deny`. Used as a stand-in when the
/// test's policy is expected to never ask (e.g. `DangerFullAccess`).
struct DenyPrompter;

impl PermissionPrompter for DenyPrompter {
    fn decide<'a>(
        &'a self,
        _request: AsyncPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>>
    {
        Box::pin(async { Ok(AsyncPermissionDecision::Deny) })
    }
}

fn make_runtime() -> ConversationRuntime<ScriptedApi, StaticToolExecutor> {
    ConversationRuntime::new(
        Session::new(),
        ScriptedApi::new(),
        StaticToolExecutor::new().register("echo", |input| Ok(format!("echoed:{input}"))),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    )
}

#[tokio::test]
async fn conversation_streaming_emits_same_logical_sequence_as_sync() {
    // Sync baseline.
    let mut sync_runtime = make_runtime();
    let sync_summary = sync_runtime.run_turn("hello", None).expect("sync turn");

    // Streaming run with a no-op prompter (policy is DangerFullAccess so
    // no prompt is needed).
    let mut streaming_runtime = make_runtime();
    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);

    // Drain concurrently so the bounded channel never stalls.
    let drain_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(block) = rx.recv().await {
            collected.push(block);
        }
        collected
    });

    let streaming_summary = streaming_runtime
        .run_turn_streaming("hello", tx, prompter)
        .await
        .expect("streaming turn");
    let blocks = drain_task.await.expect("drain");

    assert_eq!(streaming_summary.iterations, sync_summary.iterations);
    assert_eq!(
        streaming_summary.assistant_messages.len(),
        sync_summary.assistant_messages.len()
    );
    assert_eq!(
        streaming_summary.tool_results.len(),
        sync_summary.tool_results.len()
    );
    assert_eq!(
        streaming_runtime.session().messages.len(),
        sync_runtime.session().messages.len(),
        "both paths must produce identical session transcripts"
    );

    // Shape assertions: at least one text delta (with a done marker),
    // one tool call card, and a matching tool result.
    let text_count = blocks
        .iter()
        .filter(|b| matches!(b, RenderBlock::TextDelta { .. }))
        .count();
    let tool_call_count = blocks
        .iter()
        .filter(|b| matches!(b, RenderBlock::ToolCall { .. }))
        .count();
    let tool_result_count = blocks
        .iter()
        .filter(|b| matches!(b, RenderBlock::ToolResult { .. }))
        .count();
    assert!(
        text_count >= 2,
        "expected at least one delta + one done marker, got {text_count}"
    );
    assert_eq!(tool_call_count, 1);
    assert_eq!(tool_result_count, 1);

    for block in &blocks {
        if let RenderBlock::ToolCall { status, .. } = block {
            assert!(matches!(status, ToolCallStatus::Running));
        }
    }
}

#[tokio::test]
async fn conversation_streaming_success_render_uses_pure_output_before_post_hook_feedback() {
    struct EditApi {
        calls: usize,
    }

    impl ApiClient for EditApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.calls += 1;
            match self.calls {
                1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "edit-1".to_string(),
                        name: "Edit".to_string(),
                        input: r#"{"path":"/tmp/a.rs","old":"old","new":"new"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                _ => Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]),
            }
        }
    }

    let edit_output = r#"{"filePath":"/tmp/a.rs","structuredPatch":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"lines":["-old","+new"]}]}"#
        .to_string();
    let mut runtime = ConversationRuntime::new_with_features(
        Session::new(),
        EditApi { calls: 0 },
        StaticToolExecutor::new().register("Edit", move |_input| Ok(edit_output.clone())),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
        &RuntimeFeatureConfig::default().with_hooks(RuntimeHookConfig::new(
            Vec::new(),
            vec!["printf 'post hook ran'".to_string()],
            Vec::new(),
        )),
    );

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(block) = rx.recv().await {
            collected.push(block);
        }
        collected
    });
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);

    let summary = runtime
        .run_turn_streaming("edit", tx, prompter)
        .await
        .expect("streaming turn");
    let blocks = drain_task.await.expect("drain");

    let rendered_body = blocks
        .iter()
        .find_map(|block| match block {
            RenderBlock::ToolResult { body, .. } => Some(body),
            _ => None,
        })
        .expect("tool result body emitted");
    assert!(
        matches!(rendered_body, ToolResultBody::Diff(_)),
        "post hook feedback must not corrupt successful structured edit render: {rendered_body:?}"
    );

    let ContentBlock::ToolResult { output, .. } = &summary.tool_results[0].blocks[0] else {
        panic!("expected tool result block");
    };
    assert!(
        output.contains("post hook ran"),
        "model-facing output keeps post hook feedback: {output:?}"
    );
}

#[tokio::test]
async fn conversation_streaming_records_fallback_after_empty_assistant_retries() {
    struct EmptyApi;

    impl ApiClient for EmptyApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![AssistantEvent::MessageStop])
        }
    }

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        EmptyApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(block) = rx.recv().await {
            collected.push(block);
        }
        collected
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("hello", tx, prompter)
        .await
        .expect("streaming empty completions should fall back");
    let blocks = drain_task.await.expect("drain");

    assert_eq!(summary.iterations, 6);
    assert_eq!(summary.assistant_messages.len(), 1);
    // The retries persist their continuation reminders as System transcript
    // messages (stable positions keep the prefix cache intact), so assert on
    // the trailing synthetic closer rather than an exact message count.
    let last = runtime
        .session()
        .messages
        .last()
        .expect("session has messages");
    assert_eq!(last.role, runtime::session::MessageRole::Assistant);
    assert!(matches!(
        &last.blocks[0],
        ContentBlock::Text { text } if text.contains("no assistant content")
    ));
    assert!(blocks.iter().any(|block| matches!(
        block,
        RenderBlock::TextDelta { text, done: false, .. }
            if text.contains("no assistant content")
    )));
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, RenderBlock::TextDelta { done: true, .. }))
    );
}

#[tokio::test]
async fn conversation_streaming_surfaces_auto_compaction_progress() {
    struct CompactionApi;

    impl ApiClient for CompactionApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::Usage(runtime::TokenUsage {
                    input_tokens: 1_000,
                    output_tokens: 4,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 119_000,
                    output_tokens_details: None,
                }),
                AssistantEvent::MessageStop,
            ])
        }
    }

    let mut session = Session::new();
    session.messages = Arc::new(vec![
        ConversationMessage::user_text("one"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "two".to_string(),
        }]),
        ConversationMessage::user_text("three"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "four".to_string(),
        }]),
    ]);
    let mut runtime = ConversationRuntime::new(
        session,
        CompactionApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    )
    .with_auto_compaction_input_tokens_threshold(100_000);

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let drain_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(block) = rx.recv().await {
            collected.push(block);
        }
        collected
    });

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("trigger", tx, prompter)
        .await
        .expect("streaming turn");
    let blocks = drain_task.await.expect("drain");

    let event = summary
        .auto_compaction
        .expect("auto compaction fired during the streaming turn");
    assert_eq!(event.removed_message_count, 2);
    assert!(
        event.tokens_before > 0,
        "the done notice needs a real before-figure: {event:?}"
    );
    let system_texts: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            RenderBlock::System { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let started = system_texts
        .iter()
        .position(|text| text.starts_with("Compacting conversation"));
    let finished = system_texts
        .iter()
        .position(|text| text.starts_with("Compacted conversation · 2 messages summarized"));

    assert!(
        started.is_some(),
        "expected auto-compaction start notice, got {system_texts:?}"
    );
    assert!(
        finished.is_some(),
        "expected auto-compaction completion notice, got {system_texts:?}"
    );
    assert!(
        started < finished,
        "start notice should precede completion notice: {system_texts:?}"
    );
}

/// Scripted API that always produces a single tool call, so the
/// permission path is exercised even under a non-permissive policy.
struct AlwaysToolApi {
    calls: usize,
}

impl ApiClient for AlwaysToolApi {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.calls += 1;
        match self.calls {
            1 => Ok(vec![
                AssistantEvent::ToolUse {
                    id: "call-1".to_string(),
                    name: "echo".to_string(),
                    input: "ping".to_string(),
                },
                AssistantEvent::MessageStop,
            ]),
            _ => Ok(vec![
                AssistantEvent::TextDelta("wrapped".to_string()),
                AssistantEvent::MessageStop,
            ]),
        }
    }
}

#[tokio::test]
async fn conversation_streaming_permission_prompt_round_trips() {
    // WorkspaceWrite + tool requiring DangerFullAccess forces the
    // policy to route through `prompt_or_deny`, which is the branch we
    // want to exercise here. (`PermissionMode`'s derived `Ord` has
    // `Prompt > DangerFullAccess`, so using `Prompt` alone would
    // short-circuit to `Allow` before reaching the prompter.)
    let runtime_policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
        .with_tool_requirement("echo", PermissionMode::DangerFullAccess);
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        AlwaysToolApi { calls: 0 },
        StaticToolExecutor::new().register("echo", |i| Ok(format!("ok:{i}"))),
        runtime_policy,
        vec!["system".to_string()],
    );

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let (prompter, mut prompt_rx) = ChannelPrompter::new(4);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(prompter);

    // Responder task: accepts one permission prompt and allows it.
    let responder = tokio::spawn(async move {
        let (request, responder) = prompt_rx.recv().await.expect("prompt received");
        assert_eq!(request.tool, "echo");
        responder
            .respond(AsyncPermissionDecision::AllowOnce)
            .expect("responder");
    });

    let drain_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(block) = rx.recv().await {
            collected.push(block);
        }
        collected
    });

    let summary = runtime
        .run_turn_streaming("do it", tx, prompter)
        .await
        .expect("streaming turn");
    responder.await.expect("responder join");
    let blocks = drain_task.await.expect("drain");

    assert_eq!(summary.iterations, 2);
    assert_eq!(summary.tool_results.len(), 1);
    let tool_result = blocks
        .iter()
        .find_map(|b| match b {
            RenderBlock::ToolResult { is_error, .. } => Some(*is_error),
            _ => None,
        })
        .expect("tool result emitted");
    assert!(
        !tool_result,
        "allow-once should not produce an error result"
    );
}

#[tokio::test]
async fn conversation_streaming_denied_tool_settles_its_card() {
    // A denied tool must still emit a ToolResult render block bound to the
    // tool_call_id: the streaming parser already flipped the card to Running,
    // and only a ToolResult reconciles it — without one the card spun forever
    // under the denial banner.
    let runtime_policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
        .with_tool_requirement("echo", PermissionMode::DangerFullAccess);
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        AlwaysToolApi { calls: 0 },
        StaticToolExecutor::new().register("echo", |i| Ok(format!("ok:{i}"))),
        runtime_policy,
        vec!["system".to_string()],
    );

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let drain_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        while let Some(block) = rx.recv().await {
            collected.push(block);
        }
        collected
    });

    let summary = runtime
        .run_turn_streaming("do it", tx, prompter)
        .await
        .expect("streaming turn continues after denial");
    let blocks = drain_task.await.expect("drain");

    assert_eq!(summary.tool_results.len(), 1);
    let denied_result = blocks
        .iter()
        .find_map(|b| match b {
            RenderBlock::ToolResult {
                is_error,
                tool_call_id,
                ..
            } => Some((*is_error, tool_call_id.clone())),
            _ => None,
        })
        .expect("denied tool must emit a ToolResult render block");
    assert!(denied_result.0, "denial settles the card as an error");
    assert_eq!(
        denied_result.1 .0, "call-1",
        "result must bind to the spawning tool call so the card reconciles"
    );
}

struct SleepToolApi {
    calls: usize,
}

impl ApiClient for SleepToolApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.calls += 1;
        match self.calls {
            1 => Ok(vec![
                AssistantEvent::ToolUse {
                    id: "sleep-1".to_string(),
                    name: "Sleep".to_string(),
                    input: r#"{"duration_ms":150}"#.to_string(),
                },
                AssistantEvent::MessageStop,
            ]),
            _ => Ok(vec![
                AssistantEvent::TextDelta("awake".to_string()),
                AssistantEvent::MessageStop,
            ]),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_streaming_dispatches_single_safe_and_serial_tools_off_turn_future() {
    struct MixedToolApi {
        calls: usize,
    }

    impl ApiClient for MixedToolApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.calls += 1;
            match self.calls {
                1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "read-1".to_string(),
                        name: "Read".to_string(),
                        input: r#"{"path":"src/lib.rs"}"#.to_string(),
                    },
                    AssistantEvent::ToolUse {
                        id: "edit-1".to_string(),
                        name: "Edit".to_string(),
                        input: r#"{"path":"src/lib.rs","old":"a","new":"b"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                _ => Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]),
            }
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let dispatch_seen = Arc::clone(&seen);
    let dispatch: ConcurrentDispatchFn = Arc::new(move |tool_name, input| {
        dispatch_seen
            .lock()
            .expect("seen lock")
            .push(tool_name.to_string());
        std::thread::sleep(Duration::from_millis(80));
        Ok(format!("dispatch:{tool_name}:{input}"))
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        MixedToolApi { calls: 0 },
        StaticToolExecutor::new()
            .register("Read", |_input| {
                panic!("single Read must use concurrent_dispatch in streaming mode")
            })
            .register("Edit", |_input| {
                panic!("serial Edit must use concurrent_dispatch in streaming mode")
            }),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_concurrent_dispatch(dispatch);

    let (tx, _rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let turn = runtime.run_turn_streaming("tools", tx, prompter);
    tokio::pin!(turn);

    tokio::select! {
        () = tokio::time::sleep(Duration::from_millis(20)) => {}
        result = &mut turn => panic!("tool dispatch blocked the streaming future: {result:?}"),
    }

    let summary = turn.await.expect("streaming turn completes");
    assert_eq!(
        seen.lock().expect("seen lock").as_slice(),
        ["Read", "Edit"],
        "serial order must be preserved while dispatch runs off the turn future"
    );

    let outputs: Vec<String> = summary
        .tool_results
        .iter()
        .map(|message| match &message.blocks[0] {
            ContentBlock::ToolResult { output, .. } => output.clone(),
            other => panic!("expected tool result, got {other:?}"),
        })
        .collect();
    assert_eq!(
        outputs,
        vec![
            r#"dispatch:Read:{"path":"src/lib.rs"}"#.to_string(),
            r#"dispatch:Edit:{"path":"src/lib.rs","old":"a","new":"b"}"#.to_string(),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_streaming_does_not_parallelize_reads_across_ordered_tools() {
    struct EditThenReadApi {
        calls: usize,
    }

    impl ApiClient for EditThenReadApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.calls += 1;
            match self.calls {
                1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "edit-1".to_string(),
                        name: "Edit".to_string(),
                        input: r#"{"path":"src/lib.rs","old":"a","new":"b"}"#.to_string(),
                    },
                    AssistantEvent::ToolUse {
                        id: "read-1".to_string(),
                        name: "Read".to_string(),
                        input: r#"{"path":"src/lib.rs"}"#.to_string(),
                    },
                    AssistantEvent::ToolUse {
                        id: "read-2".to_string(),
                        name: "Read".to_string(),
                        input: r#"{"path":"tests/lib.rs"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                _ => Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]),
            }
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let dispatch_seen = Arc::clone(&seen);
    let dispatch: ConcurrentDispatchFn = Arc::new(move |tool_name, _input| {
        dispatch_seen
            .lock()
            .expect("seen lock")
            .push(tool_name.to_string());
        std::thread::sleep(Duration::from_millis(30));
        Ok(format!("dispatch:{tool_name}"))
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        EditThenReadApi { calls: 0 },
        StaticToolExecutor::new()
            .register("Edit", |_input| panic!("Edit should use dispatch"))
            .register("Read", |_input| panic!("Read should use dispatch")),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_concurrent_dispatch(dispatch);

    let (tx, _rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime
        .run_turn_streaming("edit then read", tx, prompter)
        .await
        .expect("streaming turn completes");

    assert_eq!(
        seen.lock().expect("seen lock").as_slice(),
        ["Edit", "Read", "Read"],
        "read-only tools after an ordered tool must not race ahead of it"
    );
}

#[tokio::test]
async fn conversation_streaming_parallelizes_all_safe_tools_and_preserves_result_order() {
    struct MultiReadApi {
        calls: usize,
    }

    impl ApiClient for MultiReadApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.calls += 1;
            match self.calls {
                1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "read-1".to_string(),
                        name: "Read".to_string(),
                        input: r#"{"path":"a.rs"}"#.to_string(),
                    },
                    AssistantEvent::ToolUse {
                        id: "read-2".to_string(),
                        name: "Read".to_string(),
                        input: r#"{"path":"b.rs"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                _ => Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]),
            }
        }
    }

    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let dispatch_active = Arc::clone(&active);
    let dispatch_max_active = Arc::clone(&max_active);
    let dispatch: ConcurrentDispatchFn = Arc::new(move |tool_name, input| {
        assert_eq!(tool_name, "Read");
        let current = dispatch_active.fetch_add(1, Ordering::SeqCst) + 1;
        dispatch_max_active.fetch_max(current, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        dispatch_active.fetch_sub(1, Ordering::SeqCst);
        Ok(format!("read:{input}"))
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        MultiReadApi { calls: 0 },
        StaticToolExecutor::new().register("Read", |_input| {
            panic!("all-safe streaming tools should use concurrent_dispatch")
        }),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_concurrent_dispatch(dispatch);

    let (tx, _rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let summary = runtime
        .run_turn_streaming("read both", tx, prompter)
        .await
        .expect("streaming turn completes");

    assert_eq!(
        max_active.load(Ordering::SeqCst),
        2,
        "all-safe streaming tools should overlap"
    );
    let outputs: Vec<String> = summary
        .tool_results
        .iter()
        .map(|message| match &message.blocks[0] {
            ContentBlock::ToolResult { output, .. } => output.clone(),
            other => panic!("expected tool result, got {other:?}"),
        })
        .collect();
    assert_eq!(
        outputs,
        vec![
            r#"read:{"path":"a.rs"}"#.to_string(),
            r#"read:{"path":"b.rs"}"#.to_string(),
        ],
        "parallel safe tools must preserve model-facing result order"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_streaming_caps_large_safe_tool_fanout() {
    struct ManyReadApi {
        calls: usize,
    }

    impl ApiClient for ManyReadApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.calls += 1;
            match self.calls {
                1 => {
                    let mut events = Vec::new();
                    for idx in 0..9 {
                        events.push(AssistantEvent::ToolUse {
                            id: format!("read-{idx}"),
                            name: "Read".to_string(),
                            input: format!(r#"{{"path":"file-{idx}.rs"}}"#),
                        });
                    }
                    events.push(AssistantEvent::MessageStop);
                    Ok(events)
                }
                _ => Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]),
            }
        }
    }

    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let dispatch_active = Arc::clone(&active);
    let dispatch_max_active = Arc::clone(&max_active);
    let dispatch: ConcurrentDispatchFn = Arc::new(move |tool_name, input| {
        assert_eq!(tool_name, "Read");
        let current = dispatch_active.fetch_add(1, Ordering::SeqCst) + 1;
        dispatch_max_active.fetch_max(current, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(20));
        dispatch_active.fetch_sub(1, Ordering::SeqCst);
        Ok(format!("read:{input}"))
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ManyReadApi { calls: 0 },
        StaticToolExecutor::new().register("Read", |_input| {
            panic!("streaming Read should use concurrent_dispatch")
        }),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_concurrent_dispatch(dispatch);

    let (tx, _rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let turn = runtime.run_turn_streaming("read many", tx, prompter);
    tokio::pin!(turn);

    tokio::select! {
        () = tokio::time::sleep(Duration::from_millis(10)) => {}
        result = &mut turn => panic!("large tool fanout blocked the streaming future: {result:?}"),
    }

    let summary = turn.await.expect("streaming turn completes");
    assert_eq!(summary.tool_results.len(), 9);
    // 9 concurrency-safe reads dispatch as an 8-wide wave + 1, never all at
    // once: the cap is `MAX_PARALLEL_SAFE_TOOL_DISPATCHES` (= 8) in
    // conversation/mod.rs. Peak in-flight is therefore exactly the cap, proving
    // the batch is both bounded (not 9 at once) and parallel (not 1 / fully
    // sequential).
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        8,
        "large safe-tool batches run in an 8-wide wave (the cap), not all 9 at once"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_streaming_sleep_yields_to_runtime() {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        SleepToolApi { calls: 0 },
        StaticToolExecutor::new().register("Sleep", |input| {
            let value: serde_json::Value = serde_json::from_str(input).expect("sleep input json");
            assert_eq!(value["duration_ms"], 150);
            assert_eq!(value["__zo_already_slept"], true);
            Ok(r#"{"duration_ms":150,"message":"Slept for 150ms"}"#.to_string())
        }),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );

    let (tx, _rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let start = Instant::now();
    let turn = runtime.run_turn_streaming("sleep", tx, prompter);
    tokio::pin!(turn);

    tokio::select! {
        () = tokio::time::sleep(Duration::from_millis(25)) => {}
        result = &mut turn => panic!("sleep turn completed too early: {result:?}"),
    }
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "blocking sleep starved the current-thread runtime"
    );

    let summary = turn.await.expect("sleep turn completes");
    assert!(start.elapsed() >= Duration::from_millis(140));
    assert_eq!(summary.tool_results.len(), 1);
}

#[tokio::test]
async fn conversation_streaming_cancels_when_receiver_dropped() {
    struct SlowApi;
    impl ApiClient for SlowApi {
        fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            // Produce enough text deltas to saturate the channel (1)
            // quickly, so the second send blocks and observes the drop.
            Ok(vec![
                AssistantEvent::TextDelta("one".to_string()),
                AssistantEvent::TextDelta("two".to_string()),
                AssistantEvent::TextDelta("three".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
    }

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        SlowApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );

    // Capacity 1 so the very first send fills the channel.
    let (tx, rx) = mpsc::channel(1);

    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);

    // Drop the receiver immediately to signal cancellation.
    drop(rx);

    let err = runtime
        .run_turn_streaming("hi", tx, prompter)
        .await
        .expect_err("should cancel");
    assert!(
        matches!(err, StreamingTurnError::Cancelled),
        "expected Cancelled, got {err:?}"
    );
}

/// Scripted API that *never* terminates the agentic loop: every stream
/// returns another tool call. Without a cap this would loop forever — the
/// fixture for `--max-turns` (`set_max_iterations`) enforcement.
struct EndlessToolApi;

impl ApiClient for EndlessToolApi {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(vec![
            AssistantEvent::ToolUse {
                id: "loop".to_string(),
                name: "echo".to_string(),
                input: "again".to_string(),
            },
            AssistantEvent::MessageStop,
        ])
    }
}

#[test]
fn set_deadline_stops_a_runaway_agent_at_the_next_iteration() {
    // A spawned sub-agent that overran its caller's wait window must stop, not
    // keep streaming (and billing) in the background. A deadline already in the
    // past trips on the first iteration boundary with the time-budget error,
    // even though the scripted API would otherwise loop forever.
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        EndlessToolApi,
        StaticToolExecutor::new().register("echo", |input| Ok(format!("echoed:{input}"))),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_deadline(std::time::Instant::now());

    // Budget exhaustion now completes the turn Ok with the work preserved and
    // the marker set (the old rollback-and-Err vaporized a cut-off agent's
    // partial work) — the loop still STOPS, which is this test's point.
    let summary = runtime
        .run_turn("go", None)
        .expect("a passed deadline must stop the loop gracefully");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::Deadline),
        "deadline must surface as the time-budget marker"
    );
    assert_eq!(summary.iterations, 1, "the loop must stop at the first boundary");
    let closer = runtime::final_assistant_text(&summary);
    assert!(
        closer.contains("Time budget"),
        "the synthetic closer must name the exhausted budget, got: {closer}"
    );
}

/// Scripted API that never terminates AND bills output tokens each turn —
/// the fixture for the output-token circuit breaker (a non-converging agentic
/// loop that keeps generating). Each stream reports `per_turn` output tokens
/// then requests another tool call.
struct TokenBurningToolApi {
    per_turn: u32,
}

impl ApiClient for TokenBurningToolApi {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(vec![
            AssistantEvent::Usage(runtime::TokenUsage {
                input_tokens: 0,
                output_tokens: self.per_turn,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens_details: None,
            }),
            AssistantEvent::ToolUse {
                id: "loop".to_string(),
                name: "echo".to_string(),
                input: "again".to_string(),
            },
            AssistantEvent::MessageStop,
        ])
    }
}

#[test]
fn turn_output_token_budget_stops_a_nonconverging_loop() {
    // The multi-day-runaway case: a loop that keeps generating without
    // converging. The iteration cap misses it when few iterations each fan out
    // huge work, so a cumulative output-token budget is the cost breaker. With
    // 400 tokens/turn and a 1000-token budget, the turn stops once the in-turn
    // output crosses the budget — gracefully, work preserved, resumable.
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        TokenBurningToolApi { per_turn: 400 },
        StaticToolExecutor::new().register("echo", |input| Ok(format!("echoed:{input}"))),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_turn_output_token_budget(Some(1000));

    let summary = runtime
        .run_turn("go", None)
        .expect("an over-budget loop must stop gracefully");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::OutputTokens),
        "crossing the output-token budget must surface as the output-token marker"
    );
    // Boundary check is at the top of each iteration against the PRIOR turns'
    // output, so it trips once accumulated output (400·n) exceeds 1000 — at the
    // start of iteration 4 (1200 > 1000), never running unbounded.
    assert!(
        summary.iterations <= 5,
        "the loop must stop promptly past the budget, got {}",
        summary.iterations
    );
    let closer = runtime::final_assistant_text(&summary);
    assert!(
        closer.contains("Output-token budget"),
        "the synthetic closer must name the exhausted budget, got: {closer}"
    );
}

/// Fixture for the input-token circuit breaker: a cache-dead loop that
/// re-sends its whole transcript at full price on every call while
/// generating almost nothing — the leak signature the output-token breaker
/// (above) structurally cannot see.
struct InputBurningToolApi {
    per_call_input: u32,
}

impl ApiClient for InputBurningToolApi {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(vec![
            AssistantEvent::Usage(runtime::TokenUsage {
                input_tokens: self.per_call_input,
                output_tokens: 10,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens_details: None,
            }),
            AssistantEvent::ToolUse {
                id: "loop".to_string(),
                name: "echo".to_string(),
                input: "again".to_string(),
            },
            AssistantEvent::MessageStop,
        ])
    }
}

#[test]
fn turn_input_token_budget_stops_a_cache_dead_loop() {
    // The observed live leak: cache reads pinned at the system prefix while
    // every call re-billed a six-figure transcript as full-price input. Output
    // stayed tiny and the wall clock was fine, so neither existing breaker
    // fired. With 200k input/call and a 500k budget, the turn must stop once
    // cumulative full-price input crosses the budget.
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        InputBurningToolApi {
            per_call_input: 200_000,
        },
        StaticToolExecutor::new().register("echo", |input| Ok(format!("echoed:{input}"))),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_turn_input_token_budget(Some(500_000));

    let summary = runtime
        .run_turn("go", None)
        .expect("an over-budget cache-dead loop must stop gracefully");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::InputTokens),
        "crossing the input-token budget must surface as the input-token marker"
    );
    assert!(
        summary.iterations <= 5,
        "the loop must stop promptly past the budget, got {}",
        summary.iterations
    );
    let closer = runtime::final_assistant_text(&summary);
    assert!(
        closer.contains("Input-token budget"),
        "the synthetic closer must name the exhausted budget, got: {closer}"
    );
}

#[test]
fn cached_input_does_not_count_toward_the_input_token_budget() {
    // The breaker meters FULL-PRICE input only: `input_tokens` excludes cache
    // reads/writes on both provider normalizations, so a healthy cached loop —
    // huge cache_read, small input — must never trip it. Pair with a small
    // iteration cap so the test terminates on the iteration budget instead.
    struct CachedInputApi;
    impl ApiClient for CachedInputApi {
        fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::Usage(runtime::TokenUsage {
                    input_tokens: 2_000,
                    output_tokens: 10,
                    cache_creation_input_tokens: 8_000,
                    cache_read_input_tokens: 190_000,
                    output_tokens_details: None,
                }),
                AssistantEvent::ToolUse {
                    id: "loop".to_string(),
                    name: "echo".to_string(),
                    input: "again".to_string(),
                },
                AssistantEvent::MessageStop,
            ])
        }
    }
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        CachedInputApi,
        StaticToolExecutor::new().register("echo", |input| Ok(format!("echoed:{input}"))),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_turn_input_token_budget(Some(500_000));
    runtime.set_max_iterations(3);

    let summary = runtime
        .run_turn("go", None)
        .expect("a healthy cached loop must stop on the iteration cap");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::Iterations),
        "cache reads/writes must not count toward the full-price input budget"
    );
}

#[test]
fn zero_turn_output_token_budget_is_unbounded_and_never_trips() {
    // A `None` budget (disabled) must never trip: the loop runs until another
    // bound stops it. Pair the burner with a small iteration cap so the test
    // terminates on the iteration budget, proving the token check stayed inert.
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        TokenBurningToolApi { per_turn: 100_000 },
        StaticToolExecutor::new().register("echo", |input| Ok(format!("echoed:{input}"))),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_turn_output_token_budget(None);
    runtime.set_max_iterations(3);

    let summary = runtime
        .run_turn("go", None)
        .expect("loop must stop on the iteration cap, not the disabled token budget");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::Iterations),
        "a disabled token budget must not pre-empt the iteration cap"
    );
}

#[test]
fn set_max_iterations_caps_the_agentic_loop() {
    // `--max-turns` on the headless `-p` path reuses `set_max_iterations` to
    // bound worst-case cost. A model that keeps requesting tools must hit the
    // cap and fail fast rather than loop unboundedly.
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        EndlessToolApi,
        StaticToolExecutor::new().register("echo", |input| Ok(format!("echoed:{input}"))),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_max_iterations(3);

    // The cap still bounds worst-case cost — the endless tool loop stops at the
    // boundary — but the turn now ends Ok with the marker and every completed
    // iteration's work (tool results included) preserved instead of rolled back.
    let summary = runtime
        .run_turn("go", None)
        .expect("an endless tool loop must trip the iteration cap gracefully");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::Iterations),
        "cap must surface as the iteration-budget marker"
    );
    assert_eq!(summary.iterations, 4, "the loop must stop right past the cap");
    assert!(
        !summary.tool_results.is_empty(),
        "the capped turn's tool results must be preserved, not rolled back"
    );
    let closer = runtime::final_assistant_text(&summary);
    assert!(
        closer.contains("Iteration budget"),
        "the synthetic closer must name the exhausted budget, got: {closer}"
    );

    // The cap must leave the session well-formed: every tool_use already has a
    // matching tool_result, so the consistency view shares the stored Arc (the
    // zero-alloc happy path) instead of sealing orphans. A regression that
    // tripped the cap mid-tool-use would brick the session on the next request.
    let stored = Arc::clone(&runtime.session().messages);
    let consistent = runtime.session().tool_consistent_messages();
    assert!(
        Arc::ptr_eq(&stored, &consistent),
        "iteration cap must not leave orphaned tool_use blocks in the session"
    );
}

/// W: TurnEnd(Stop) 훅의 followup Stop-loop가 **스트리밍 경로에도** 배선됐는지
/// 고정한다 — 종전엔 sync `run_turn`에만 있어 인터랙티브 TUI에서 Stop 훅이
/// 죽은 기능이었다. sync 테스트(`stop_hook_followup_reinjects_until_bounded`)
/// 의 미러: 모델은 매 턴 깨끗이 멈추고, 훅은 항상 계속을 요구하며, 루프는
/// `max_stop_loops`에서 반드시 멈춘다.
#[tokio::test]
async fn streaming_stop_hook_followup_reinjects_until_bounded() {
    struct AlwaysStopApi;
    impl ApiClient for AlwaysStopApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("done".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
    }

    #[cfg(windows)]
    let hook =
        r#"printf '{"hookSpecificOutput":{"followupMessage":"keep going"}}'"#.replace('\'', "\"");
    #[cfg(not(windows))]
    let hook = r#"printf '{"hookSpecificOutput":{"followupMessage":"keep going"}}'"#.to_string();

    let mut runtime = ConversationRuntime::new_with_features(
        Session::new(),
        AlwaysStopApi,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
        &runtime::RuntimeFeatureConfig::default()
            .with_hooks(runtime::RuntimeHookConfig::default().with_turn_end(vec![hook])),
    );
    runtime.set_max_stop_loops(2);

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let drain_task = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    runtime
        .run_turn_streaming_maybe_deep("start", Vec::new(), tx, prompter)
        .await
        .expect("streaming stop-loop turn should succeed");
    drain_task.await.expect("drain");

    // 1 최초 유저 턴 + 정확히 max_stop_loops(2)회 재주입. 재주입이 없으면 1,
    // 바운드가 없으면 영영 안 멈춘다.
    let user_turns = runtime
        .session()
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .count();
    assert_eq!(
        user_turns, 3,
        "1 initial + 2 bounded streaming continuations"
    );
}

/// Sync/headless `run_turn` path: a Fable safety-classifier refusal
/// (`stop_reason: "refusal"`) must fall back once to the current Opus catalog head, drop the refused
/// partial from history, and record the fallback model's answer. This mirrors
/// the streaming-seam coverage for the non-TUI loop (headless `-p` and
/// spawned sub-agents both drive `run_turn`).
#[test]
fn sync_refusal_on_fable_falls_back_to_opus_and_retries_once() {
    struct SyncRefusalThenAnswerApi {
        seen_overrides: Arc<Mutex<Vec<Option<String>>>>,
    }
    impl ApiClient for SyncRefusalThenAnswerApi {
        fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            let call = {
                let mut seen = self.seen_overrides.lock().expect("lock");
                seen.push(request.model_override.clone());
                seen.len()
            };
            if call == 1 {
                Ok(vec![
                    AssistantEvent::TextDelta("REFUSED-SYNC-PARTIAL".to_string()),
                    AssistantEvent::StopReason("refusal".to_string()),
                    AssistantEvent::MessageStop,
                ])
            } else {
                Ok(vec![
                    AssistantEvent::TextDelta("opus sync answer".to_string()),
                    AssistantEvent::StopReason("end_turn".to_string()),
                    AssistantEvent::MessageStop,
                ])
            }
        }
    }

    let seen_overrides = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        SyncRefusalThenAnswerApi {
            seen_overrides: Arc::clone(&seen_overrides),
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-fable-5");

    let summary = runtime
        .run_turn("hi", None)
        .expect("fable refusal should fall back to opus and complete");

    // Two calls: the refused Fable turn, then the current Opus-head retry.
    let seen = seen_overrides.lock().expect("lock").clone();
    assert_eq!(
        seen,
        vec![None, Some(api::latest_anthropic_model().to_string())],
        "the retry must carry the current Opus catalog override; got {seen:?}"
    );
    assert_eq!(summary.iterations, 2);

    // Refused partial dropped; Opus answer recorded.
    let history: String = runtime
        .session()
        .messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !history.contains("REFUSED-SYNC-PARTIAL"),
        "refused partial must not persist, got {history:?}"
    );
    assert!(
        history.contains("opus sync answer"),
        "fallback answer must be recorded, got {history:?}"
    );
}

/// Sync path: a refusal already on Opus (or any non-Fable Claude) gets exactly
/// one retry on the same model — the safety classifier is sample-dependent, so a
/// second draw usually clears a false positive — and is then surfaced. Bounded:
/// the second refusal ends the turn instead of looping.
#[test]
fn sync_refusal_on_opus_retries_once_then_surfaces() {
    struct AlwaysRefuseApi {
        calls: Arc<Mutex<usize>>,
    }
    impl ApiClient for AlwaysRefuseApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            *self.calls.lock().expect("lock") += 1;
            Ok(vec![
                AssistantEvent::StopReason("refusal".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
    }

    let calls = Arc::new(Mutex::new(0usize));
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        AlwaysRefuseApi {
            calls: Arc::clone(&calls),
        },
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model(api::latest_anthropic_model());

    let summary = runtime
        .run_turn("hi", None)
        .expect("an opus refusal should surface, not error");

    assert_eq!(
        *calls.lock().expect("lock"),
        2,
        "one same-model retry, then surface"
    );
    // The turn is well-formed: it ended with a recorded assistant notice.
    assert!(!summary.assistant_messages.is_empty());
}

// ── Verification-treadmill circuit breaker ──────────────────────────────────
//
// A turn that keeps re-planning / re-validating / re-spawning (Workflow,
// WorkflowValidate, SpawnMultiAgent, Agent) with a NEW spec each round but never
// changes a file slips past the repetition guard (each fingerprint differs). The
// treadmill guard counts verify-class rounds with no file mutation and stops the
// turn gracefully. These tests drive the sync (`run_turn`) and streaming loops
// with a scripted tool loop and the `ZO_VERIFY_TREADMILL_ROUNDS` knob.

/// Serializes the treadmill tests, which share the process-global
/// `ZO_VERIFY_TREADMILL_ROUNDS`, and restores the prior value on drop. Only a
/// verify-class round reads the var, so no other test in this binary is affected.
static VERIFY_TREADMILL_ENV_LOCK: Mutex<()> = Mutex::new(());

struct VerifyTreadmillEnv {
    _guard: std::sync::MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
}

impl VerifyTreadmillEnv {
    fn set(value: &str) -> Self {
        let guard = VERIFY_TREADMILL_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev = std::env::var_os("ZO_VERIFY_TREADMILL_ROUNDS");
        std::env::set_var("ZO_VERIFY_TREADMILL_ROUNDS", value);
        Self {
            _guard: guard,
            prev,
        }
    }
}

impl Drop for VerifyTreadmillEnv {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var("ZO_VERIFY_TREADMILL_ROUNDS", v),
            None => std::env::remove_var("ZO_VERIFY_TREADMILL_ROUNDS"),
        }
    }
}

/// Scripted API that never terminates and cycles through a fixed list of tool
/// names, one call per stream, with a DISTINCT input each round (so the
/// identical-call repetition guard never fires — only the class-based treadmill
/// guard can catch this loop). The fixture for the verification treadmill.
struct ScriptedToolLoopApi {
    script: Vec<&'static str>,
    call: usize,
}

impl ScriptedToolLoopApi {
    fn new(script: Vec<&'static str>) -> Self {
        Self { script, call: 0 }
    }
}

impl ApiClient for ScriptedToolLoopApi {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let name = self.script[self.call % self.script.len()];
        self.call += 1;
        Ok(vec![
            AssistantEvent::ToolUse {
                id: format!("call-{}", self.call),
                name: name.to_string(),
                // Distinct spec every round: a real treadmill re-plans with new
                // arguments, so the repetition fingerprint never matches.
                input: format!("{{\"round\":{}}}", self.call),
            },
            AssistantEvent::MessageStop,
        ])
    }
}

fn verify_treadmill_executor() -> StaticToolExecutor {
    StaticToolExecutor::new()
        .register("Workflow", |_input| Ok("planned".to_string()))
        .register("WorkflowValidate", |_input| Ok("validated".to_string()))
        .register("SpawnMultiAgent", |_input| Ok("spawned".to_string()))
        .register("Agent", |_input| Ok("delegated".to_string()))
        .register("edit_file", |_input| Ok("edited".to_string()))
        .register("Read", |_input| Ok("contents".to_string()))
}

#[test]
fn verify_treadmill_hard_stops_a_no_edit_spawn_loop() {
    // Soft=3, hard=7: a turn that spawns/verifies every round without changing a
    // file trips the hard stop at round 7 — gracefully, work preserved, resumable
    // — even though each call carries a new spec that the repetition guard misses.
    let _env = VerifyTreadmillEnv::set("3");
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedToolLoopApi::new(vec!["SpawnMultiAgent"]),
        verify_treadmill_executor(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );

    let summary = runtime
        .run_turn("go", None)
        .expect("a verification treadmill must stop gracefully, not error");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::VerificationTreadmill),
        "a no-edit verify loop must surface as the verification-treadmill marker"
    );
    assert_eq!(
        summary.iterations, 7,
        "hard = soft(3) + 4 = 7, so the loop stops at round 7"
    );
    assert!(
        !summary.tool_results.is_empty(),
        "the work up to the cutoff must be preserved"
    );
    let closer = runtime::final_assistant_text(&summary);
    assert!(
        closer.contains("self-verification loop"),
        "the closer must be the CC-style handback, got: {closer}"
    );
}

#[test]
fn verify_treadmill_resets_on_an_interleaved_file_edit() {
    // Soft=3, hard=7, but every third round edits a file. A file mutation is real
    // progress and resets the tally, so it never climbs past 2 and the loop stops
    // on the iteration cap instead — proving an interleaved edit clears the run.
    let _env = VerifyTreadmillEnv::set("3");
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedToolLoopApi::new(vec!["Workflow", "Workflow", "edit_file"]),
        verify_treadmill_executor(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_max_iterations(12);

    let summary = runtime
        .run_turn("go", None)
        .expect("the loop must stop on the iteration cap, not the treadmill");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::Iterations),
        "an interleaved edit resets the tally, so the treadmill must never trip"
    );
}

#[test]
fn verify_treadmill_never_fires_on_a_pure_research_loop() {
    // Soft=1 (maximally aggressive), yet a turn that only reads/greps — never a
    // verify-class tool — can NEVER trip the guard (the core invariant). It stops
    // on the iteration cap.
    let _env = VerifyTreadmillEnv::set("1");
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedToolLoopApi::new(vec!["Read"]),
        verify_treadmill_executor(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_max_iterations(6);

    let summary = runtime
        .run_turn("go", None)
        .expect("a pure research loop must stop on the iteration cap");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::Iterations),
        "a loop with no verify-class tool must never surface the treadmill marker"
    );
}

#[test]
fn verify_treadmill_disabled_by_zero_env_never_fires() {
    // ZO_VERIFY_TREADMILL_ROUNDS=0 disables the whole guard: a no-edit spawn
    // loop that would otherwise hard-stop now runs until another bound (the
    // iteration cap) stops it, proving the treadmill check stayed inert.
    let _env = VerifyTreadmillEnv::set("0");
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedToolLoopApi::new(vec!["Workflow"]),
        verify_treadmill_executor(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_max_iterations(8);

    let summary = runtime
        .run_turn("go", None)
        .expect("with the guard disabled the loop stops on the iteration cap");
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::Iterations),
        "a disabled treadmill must not pre-empt the iteration cap"
    );
}

#[tokio::test]
async fn verify_treadmill_hard_stops_the_streaming_loop_too() {
    // The streaming loop shares `note_verify_treadmill`; assert the seam is wired
    // there as well. Soft=2, hard=6: a no-edit Workflow loop stops at round 6.
    let _env = VerifyTreadmillEnv::set("2");
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedToolLoopApi::new(vec!["Workflow"]),
        verify_treadmill_executor(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let summary = runtime
        .run_turn_streaming("go", tx, prompter)
        .await
        .expect("a streaming verification treadmill must stop gracefully");
    drain.await.expect("drain");

    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::VerificationTreadmill),
        "the streaming loop must honour the treadmill guard"
    );
    assert_eq!(
        summary.iterations, 6,
        "hard = soft(2) + 4 = 6, so the streaming loop stops at round 6"
    );
}

/// Scripted API that emits each `prefix` tool once and then `tail` forever, with
/// a distinct input every round. Models the shape the treadmill guard misreads:
/// real edits early, then a long run of delegation/verification rounds.
struct ScriptedPrefixThenLoopApi {
    prefix: Vec<&'static str>,
    tail: &'static str,
    call: usize,
}

impl ScriptedPrefixThenLoopApi {
    fn new(prefix: Vec<&'static str>, tail: &'static str) -> Self {
        Self {
            prefix,
            tail,
            call: 0,
        }
    }
}

impl ApiClient for ScriptedPrefixThenLoopApi {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let name = self.prefix.get(self.call).copied().unwrap_or(self.tail);
        self.call += 1;
        Ok(vec![
            AssistantEvent::ToolUse {
                id: format!("call-{}", self.call),
                name: name.to_string(),
                input: format!("{{\"round\":{}}}", self.call),
            },
            AssistantEvent::MessageStop,
        ])
    }
}

#[test]
fn verify_treadmill_spares_a_turn_that_already_edited_files_exactly_once() {
    // The orchestrator false positive the guard's own doc admits: a turn that
    // does real work and then delegates/verifies looks identical to a no-edit
    // spawn loop, and the host refuses to auto-continue a treadmill stop — so
    // the turn was lost outright. It now costs ONE extra advisory round.
    //
    // Soft=2, hard=6. Round 1 edits a file (progress banked, tally reset), then
    // Workflow forever: the tally reaches 6 at round 7 and is spared (rewound to
    // the soft threshold), reaches 6 again at round 11, and stops for real
    // because no NEW progress landed in between. Before the fix this stopped at
    // round 7.
    let _env = VerifyTreadmillEnv::set("2");
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedPrefixThenLoopApi::new(vec!["edit_file"], "Workflow"),
        verify_treadmill_executor(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_max_iterations(20);

    let summary = runtime
        .run_turn("go", None)
        .expect("a spared treadmill must still stop gracefully, not error");

    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::VerificationTreadmill),
        "the guard must still stop the turn once its single nudge is spent"
    );
    assert_eq!(
        summary.iterations, 11,
        "the productive turn earns one rewind to the soft threshold (stop at 7 → 11)"
    );
}

/// Emits one successful `edit_file`, then the SAME `bash` call with identical
/// input forever — the shape that arms the per-turn repetition hard stop on a
/// turn which has already produced real work.
struct EditThenRepeatedProbeApi {
    call: usize,
}

impl ApiClient for EditThenRepeatedProbeApi {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.call += 1;
        let (name, input) = if self.call == 1 {
            ("edit_file", r#"{"path":"x.rs"}"#)
        } else {
            ("bash", r#"{"command":"cargo test -p runtime"}"#)
        };
        Ok(vec![
            AssistantEvent::ToolUse {
                id: format!("call-{}", self.call),
                name: name.to_string(),
                input: input.to_string(),
            },
            AssistantEvent::MessageStop,
        ])
    }
}

#[test]
fn repetition_hard_stop_spares_a_productive_turn_on_the_live_preflight_path() {
    // The reprieve has to land on the path the turn loop ACTUALLY takes. Once a
    // stop is armed the repeat is skipped before dispatch by
    // `next_tool_repetition_hard_stop_notice`, so `note_tool_repetition` never
    // runs for it — a fix wired only into the latter is dead code here, and a
    // test that calls it directly cannot tell the difference.
    //
    // Round 1 edits a file (progress banked). Rounds 2-4 issue the same probe
    // (Ok, Ok, advise-at-3), arming the stop. Round 5 hits the armed preflight
    // and is spared; round 6 hits it again with no NEW progress and ends the
    // turn, so the loop breaks entering round 7 and settles at 6 iterations.
    // Before the fix round 5 terminated and it settled at 5.
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        EditThenRepeatedProbeApi { call: 0 },
        StaticToolExecutor::new()
            .register("edit_file", |_input| Ok("edited".to_string()))
            .register("bash", |_input| Ok("probe output".to_string())),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_max_iterations(30);

    let summary = runtime.run_turn("go", None).expect("the turn completes");

    assert_eq!(
        summary.iterations, 6,
        "a turn that edited a file earns exactly one extra round past the armed \
         repetition stop (5 before the fix, 6 after)"
    );
    // The stop reports itself. Returning `None` here made the host read a turn
    // the harness had to kill as a converged one and call `clear_turn_failure()`,
    // resetting the escalation ladder and grind streak this pattern should feed.
    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::ToolRepetition),
        "the repetition guard must name itself like every other non-convergent stop"
    );
    // ...and the transcript ends well-formed. Without the closer the turn ended
    // on tool results, which serialize as wire role `user`.
    let last = runtime
        .session()
        .messages
        .last()
        .expect("session has messages");
    assert_eq!(
        last.role,
        runtime::session::MessageRole::Assistant,
        "a repetition-stopped turn must still close on an assistant message"
    );
    let closer = runtime::final_assistant_text(&summary);
    assert!(
        closer.contains("Tool-repetition guard"),
        "the closer must name the guard that stopped the turn, got: {closer}"
    );
}

/// Alternates a verify-class `Workflow` round with an `edit_file` that always
/// FAILS — a loop that keeps attempting the same impossible write.
struct WorkflowThenFailingEditApi {
    call: usize,
}

impl ApiClient for WorkflowThenFailingEditApi {
    fn stream(&mut self, _req: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.call += 1;
        let name = if self.call % 2 == 1 {
            "Workflow"
        } else {
            "edit_file"
        };
        Ok(vec![
            AssistantEvent::ToolUse {
                id: format!("call-{}", self.call),
                name: name.to_string(),
                input: format!("{{\"round\":{}}}", self.call),
            },
            AssistantEvent::MessageStop,
        ])
    }
}

#[test]
fn verify_treadmill_is_not_reset_by_an_edit_that_failed() {
    // The counter may only be forgiven by work that LANDED. Reading the
    // requested tool names instead let a failed — or permission-denied — edit
    // reset the tally every other round, so this loop made no progress at all
    // yet evaded the guard forever and ran to the iteration cap.
    let _env = VerifyTreadmillEnv::set("2");
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        WorkflowThenFailingEditApi { call: 0 },
        StaticToolExecutor::new()
            .register("Workflow", |_input| Ok("planned".to_string()))
            .register("edit_file", |_input| {
                Err(runtime::ToolError::new("permission denied"))
            }),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_max_iterations(20);

    let summary = runtime.run_turn("go", None).expect("the turn completes");

    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::VerificationTreadmill),
        "a failing edit is not progress, so the treadmill must still fire — \
         reaching the iteration cap instead means the guard was evaded"
    );
}

#[test]
fn verify_treadmill_nudge_is_not_granted_twice_to_one_turn() {
    // The bound that keeps the reprieve from becoming an escape hatch: banking
    // more edits later does NOT buy a second nudge, because the nudge budget is
    // one per turn and shared across every behavior heuristic. Edits at rounds 1
    // and 8 would each look like "fresh progress", yet the turn still stops at
    // round 11 — the same round as the single-edit case above.
    let _env = VerifyTreadmillEnv::set("2");
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        ScriptedPrefixThenLoopApi::new(
            vec![
                "edit_file",
                "Workflow",
                "Workflow",
                "Workflow",
                "Workflow",
                "Workflow",
                "Workflow",
                "edit_file",
            ],
            "Workflow",
        ),
        verify_treadmill_executor(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_max_iterations(30);

    let summary = runtime
        .run_turn("go", None)
        .expect("the second stop must be graceful too");

    assert_eq!(
        summary.budget_exhausted,
        Some(runtime::BudgetExhausted::VerificationTreadmill),
        "a second burst of edits must not buy a second reprieve"
    );
}

/// Several spawns in one message run side by side — the model asked for them
/// together, as Claude Code runs its `Task` calls — and the shell the model
/// put after them starts only once both have finished. Before, each `Agent`
/// call waited for the previous subagent to finish ("병렬로 돌리지 않음",
/// 2026-09-03).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conversation_streaming_runs_spawns_of_one_message_side_by_side() {
    struct SpawnsThenShellApi {
        calls: usize,
    }

    impl ApiClient for SpawnsThenShellApi {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.calls += 1;
            match self.calls {
                1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "spawn-a".to_string(),
                        name: "Agent".to_string(),
                        input: r#"{"description":"slice a","prompt":"do a"}"#.to_string(),
                    },
                    AssistantEvent::ToolUse {
                        id: "spawn-b".to_string(),
                        name: "Agent".to_string(),
                        input: r#"{"description":"slice b","prompt":"do b"}"#.to_string(),
                    },
                    AssistantEvent::ToolUse {
                        id: "shell-1".to_string(),
                        name: "Bash".to_string(),
                        input: r#"{"command":"echo after"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                _ => Ok(vec![
                    AssistantEvent::TextDelta("done".to_string()),
                    AssistantEvent::MessageStop,
                ]),
            }
        }
    }

    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let finished_spawns = Arc::new(AtomicUsize::new(0));
    let spawns_done_when_shell_ran = Arc::new(AtomicUsize::new(usize::MAX));
    let (dispatch_active, dispatch_max, dispatch_finished, dispatch_shell) = (
        Arc::clone(&active),
        Arc::clone(&max_active),
        Arc::clone(&finished_spawns),
        Arc::clone(&spawns_done_when_shell_ran),
    );
    let dispatch: ConcurrentDispatchFn = Arc::new(move |tool_name, input| {
        if tool_name == "Agent" {
            let current = dispatch_active.fetch_add(1, Ordering::SeqCst) + 1;
            dispatch_max.fetch_max(current, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(80));
            dispatch_active.fetch_sub(1, Ordering::SeqCst);
            dispatch_finished.fetch_add(1, Ordering::SeqCst);
            assert!(
                input.contains("__zo_tool_call_id"),
                "a spawn in a wave carries its tool_use id like one run alone: {input}"
            );
        } else {
            dispatch_shell.store(dispatch_finished.load(Ordering::SeqCst), Ordering::SeqCst);
        }
        Ok(format!("dispatch:{tool_name}"))
    });

    let mut runtime = ConversationRuntime::new(
        Session::new(),
        SpawnsThenShellApi { calls: 0 },
        StaticToolExecutor::new()
            .register("Agent", |_input| panic!("Agent should use dispatch"))
            .register("Bash", |_input| panic!("Bash should use dispatch")),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_concurrent_dispatch(dispatch);

    let (tx, _rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(DenyPrompter);
    runtime
        .run_turn_streaming("fan out, then check", tx, prompter)
        .await
        .expect("streaming turn completes");

    assert_eq!(
        max_active.load(Ordering::SeqCst),
        2,
        "both spawns must be in flight at once"
    );
    assert_eq!(
        spawns_done_when_shell_ran.load(Ordering::SeqCst),
        2,
        "the shell after the spawns starts only once both have finished"
    );
}
