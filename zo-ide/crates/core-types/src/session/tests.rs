use super::{
    cleanup_rotated_logs, reconcile_tool_history, rotate_session_file_if_needed,
    seal_orphan_tool_uses, vault_path_for, AnchorSummary, ContentBlock, ConversationMessage,
    MessageRole, Session, SessionError, SessionFork,
};
use crate::json::JsonValue;
use crate::usage::TokenUsage;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn tool_result_images_survive_json_round_trip() {
    let block = ContentBlock::ToolResult {
        tool_use_id: "tu-1".into(),
        tool_name: "read_image".into(),
        output: "staged".into(),
        is_error: false,
        images: vec![
            ("image/png".into(), "QUJD".into()),
            ("image/jpeg".into(), "ZGVm".into()),
        ],
    };
    let restored = ContentBlock::from_json(&block.to_json()).expect("round-trip");
    assert_eq!(restored, block);
}

#[test]
fn text_only_tool_result_omits_images_key_for_byte_identical_back_compat() {
    let block = ContentBlock::ToolResult {
        tool_use_id: "tu-2".into(),
        tool_name: "bash".into(),
        output: "ok".into(),
        is_error: false,
        images: Vec::new(),
    };
    let json = block.to_json();
    assert!(
        json.as_object().is_some_and(|o| !o.contains_key("images")),
        "empty images must not be serialized (old JSON stays byte-identical)"
    );
    // And an older record with no `images` key still loads as empty.
    let restored = ContentBlock::from_json(&json).expect("round-trip");
    assert_eq!(restored, block);
}

/// The per-turn model stamp (cost attribution) must survive the JSONL
/// round-trip, and a record without it (pre-field history) must load as
/// `None` rather than failing.
#[test]
fn model_stamp_round_trips_and_is_optional() {
    let stamped = ConversationMessage::assistant(vec![ContentBlock::Text {
        text: "answer".to_string(),
    }])
    .with_model(Some("gpt-5.6-sol".to_string()));
    let json = stamped.to_json();
    let restored = ConversationMessage::from_json(&json).expect("round-trip");
    assert_eq!(restored.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(restored, stamped);

    let unstamped = ConversationMessage::user_text("hello");
    let json = unstamped.to_json();
    assert!(
        json.as_object().is_some_and(|o| !o.contains_key("model")),
        "absent model must not serialize a key (old JSON stays byte-identical)"
    );
    let restored = ConversationMessage::from_json(&json).expect("round-trip");
    assert_eq!(restored.model, None);
}

#[test]
fn assistant_usage_output_tokens_details_round_trips_in_session_row() {
    let mut session = Session::new();
    session
        .push_message(ConversationMessage::assistant_with_usage(
            vec![ContentBlock::Text {
                text: "answer".to_string(),
            }],
            Some(TokenUsage {
                input_tokens: 25,
                output_tokens: 348,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens_details: Some(crate::usage::OutputTokensDetails {
                    thinking_tokens: 312,
                }),
            }),
        ))
        .expect("assistant message should append");

    let path = temp_session_path("output-token-details");
    session.save_to_path(&path).expect("session should save");
    let contents = fs::read_to_string(&path).expect("session row should be readable");
    let row: serde_json::Value = serde_json::from_str(
        contents
            .lines()
            .find(|line| line.contains(r#""type":"message""#))
            .expect("assistant message row should exist"),
    )
    .expect("assistant message row should be JSON");
    let restored = Session::load_from_path(&path).expect("new session row should reload");
    fs::remove_file(&path).expect("temp session should be removable");
    let detailed_row_bytes = session.messages[0].to_json().render().len();
    let mut without_details = session.messages[0].clone();
    without_details
        .usage
        .as_mut()
        .expect("assistant usage")
        .output_tokens_details = None;
    let row_delta = detailed_row_bytes - without_details.to_json().render().len();

    assert_eq!(
        row["message"]["usage"]["output_tokens_details"]["thinking_tokens"],
        312
    );
    assert_eq!(restored.messages[0], session.messages[0]);
    assert_eq!(row_delta, 48, "nested output detail adds exactly 48 bytes");
}

#[test]
fn persists_and_restores_session_jsonl() {
    let mut session = Session::new();
    session
        .push_user_text("hello")
        .expect("user message should append");
    session
        .push_message(ConversationMessage::assistant_with_usage(
            vec![
                ContentBlock::Text {
                    text: "thinking".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "bash".to_string(),
                    input: "echo hi".to_string(),
                },
            ],
            Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 4,
                cache_creation_input_tokens: 1,
                cache_read_input_tokens: 2,
                output_tokens_details: None,
            }),
        ))
        .expect("assistant message should append");
    session
        .push_message(ConversationMessage::tool_result(
            "tool-1", "bash", "hi", false,
        ))
        .expect("tool result should append");

    let path = temp_session_path("jsonl");
    session.save_to_path(&path).expect("session should save");
    let restored = Session::load_from_path(&path).expect("session should load");
    fs::remove_file(&path).expect("temp file should be removable");

    assert_eq!(restored, session);
    assert_eq!(restored.messages[2].role, MessageRole::Tool);
    assert_eq!(
        restored.messages[1].usage.expect("usage").total_tokens(),
        17
    );
    assert_eq!(restored.session_id, session.session_id);
}

#[test]
fn torn_trailing_line_is_dropped_not_session_bricking() {
    // A crash/OOM/power-loss between an incremental append's write and its
    // newline leaves a partial final record. Resume must recover the complete
    // history before it instead of failing the whole session (data-loss guard).
    let mut session = Session::new();
    session.push_user_text("turn 0").expect("append");
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "turn 1".to_string(),
        }]))
        .expect("append");

    let path = temp_session_path("torn-trailing-jsonl");
    session.save_to_path(&path).expect("session should save");

    // Simulate a torn append: an incomplete JSON object with no trailing newline.
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("reopen for append");
        write!(file, "{{\"type\":\"message\",\"turn_index\":2,\"message\":")
            .expect("write torn line");
    }

    let restored =
        Session::load_from_path(&path).expect("torn trailing line must not brick resume");
    fs::remove_file(&path).expect("temp file should be removable");

    assert_eq!(
        restored, session,
        "recovered session equals the pre-crash state"
    );
    assert_eq!(restored.messages.len(), 2);
}

#[test]
fn interior_corrupt_line_still_hard_fails() {
    // Only a torn *trailing* line is recoverable. A corrupt interior line is
    // genuine damage (not a torn append) and must not be silently skipped —
    // that would drop a message and misalign every later turn index.
    let mut session = Session::new();
    session.push_user_text("turn 0").expect("append");
    session.push_user_text("turn 1").expect("append");

    let path = temp_session_path("interior-corrupt-jsonl");
    session.save_to_path(&path).expect("session should save");

    let snapshot = fs::read_to_string(&path).expect("read snapshot");
    let mut lines: Vec<&str> = snapshot.lines().collect();
    let last = lines.len().saturating_sub(1);
    lines.insert(last, "{ this is not valid json");
    fs::write(&path, format!("{}\n", lines.join("\n"))).expect("write corrupted snapshot");

    let error = Session::load_from_path(&path).expect_err("interior corruption must fail");
    fs::remove_file(&path).expect("temp file should be removable");
    assert!(error.to_string().contains("invalid JSONL record"));
}

#[test]
fn wholly_corrupt_file_is_rejected_not_recovered_as_empty() {
    // Torn-trailing recovery preserves the history *before* the torn line. A file
    // whose only content is a corrupt line has no prior history to preserve, so it
    // must be rejected as corrupt (the caller skips it) rather than silently
    // "recovered" into a valid-looking empty session.
    let path = temp_session_path("wholly-corrupt-jsonl");
    fs::write(&path, "{ this is not valid json").expect("write corrupt file");
    let error =
        Session::load_from_path(&path).expect_err("a single corrupt line must not recover");
    fs::remove_file(&path).expect("temp file should be removable");
    assert!(error.to_string().contains("invalid JSONL record"));
}

#[test]
fn jsonl_partial_load_from_turn_keeps_boundary_safe_suffix() {
    let mut session = Session::new();
    session.push_user_text("turn 0").expect("user append");
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "turn 1".to_string(),
        }]))
        .expect("assistant append");
    session.push_user_text("turn 2").expect("user append");

    let path = temp_session_path("from-turn-jsonl");
    session.save_to_path(&path).expect("session should save");
    let restored = Session::load_from_path_from_turn(&path, Some(2)).expect("partial load");
    fs::remove_file(&path).expect("temp file should be removable");

    assert_eq!(restored.messages.len(), 1);
    assert_eq!(restored.messages[0].role, MessageRole::User);
}

#[test]
fn jsonl_partial_load_refuses_compacted_prefix() {
    let mut session = Session::new();
    session.push_user_text("old").expect("append");
    session.push_user_text("kept").expect("append");
    session.record_compaction("summary", 1);

    let path = temp_session_path("from-turn-compacted-jsonl");
    session.save_to_path(&path).expect("session should save");
    let error = Session::load_from_path_from_turn(&path, Some(0)).expect_err("must refuse");
    fs::remove_file(&path).expect("temp file should be removable");

    assert!(error.to_string().contains("predates compacted history"));
}

#[test]
fn compacted_jsonl_snapshot_writes_absolute_turn_indexes() {
    let mut session = Session::new();
    session.push_user_text("old 0").expect("append");
    session.push_user_text("old 1").expect("append");
    let kept = ConversationMessage::user_text("kept 2");
    session.messages = std::sync::Arc::new(vec![kept]);
    session.record_compaction("summary", 2);

    let path = temp_session_path("from-turn-absolute-compacted-jsonl");
    session.save_to_path(&path).expect("session should save");
    let snapshot = fs::read_to_string(&path).expect("snapshot");
    assert!(
        snapshot.contains(r#""turn_index":2"#),
        "compacted snapshots must preserve absolute turn indexes: {snapshot}"
    );

    let restored = Session::load_from_path_from_turn(&path, Some(2)).expect("partial load");
    fs::remove_file(&path).expect("temp file should be removable");

    assert_eq!(restored.messages.len(), 1);
    assert_eq!(restored.messages[0].role, MessageRole::User);
}

#[test]
fn jsonl_partial_load_refuses_orphan_tool_result_start() {
    let mut session = Session::new();
    session.push_user_text("use tool").expect("append");
    session
        .push_message(assistant_tool_use("tool-1", "bash"))
        .expect("assistant append");
    session
        .push_message(ConversationMessage::tool_result(
            "tool-1", "bash", "ok", false,
        ))
        .expect("tool result append");

    let path = temp_session_path("from-turn-tool-result-jsonl");
    session.save_to_path(&path).expect("session should save");
    let error = Session::load_from_path_from_turn(&path, Some(2)).expect_err("must refuse");
    fs::remove_file(&path).expect("temp file should be removable");

    assert!(error.to_string().contains("orphaned tool_result"));
}

fn assistant_tool_use(id: &str, name: &str) -> ConversationMessage {
    ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input: "{}".to_string(),
    }])
}

#[test]
fn tool_consistent_messages_shares_arc_when_no_tool_use() {
    let mut session = Session::new();
    session.push_user_text("hi").expect("user append");
    let view = session.tool_consistent_messages();
    assert!(std::sync::Arc::ptr_eq(&session.messages, &view));
}

#[test]
fn tool_consistent_messages_shares_arc_when_result_present() {
    let mut session = Session::new();
    session.push_user_text("hi").expect("user append");
    session
        .push_message(assistant_tool_use("tool-1", "bash"))
        .expect("assistant append");
    session
        .push_message(ConversationMessage::tool_result(
            "tool-1", "bash", "ok", false,
        ))
        .expect("result append");
    let view = session.tool_consistent_messages();
    assert!(std::sync::Arc::ptr_eq(&session.messages, &view));
}

fn known(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| (*s).to_string()).collect()
}

fn reconcile(
    messages: &std::sync::Arc<Vec<ConversationMessage>>,
    known_names: &BTreeSet<String>,
) -> std::sync::Arc<Vec<ConversationMessage>> {
    reconcile_tool_history(messages, known_names, known_names)
}

fn count_blocks(messages: &[ConversationMessage], f: impl Fn(&ContentBlock) -> bool) -> usize {
    messages
        .iter()
        .flat_map(|m| &m.blocks)
        .filter(|b| f(b))
        .count()
}

#[test]
fn reconcile_shares_arc_when_all_tools_known() {
    let mut session = Session::new();
    session.push_user_text("hi").expect("user");
    session
        .push_message(assistant_tool_use("t1", "bash"))
        .expect("assistant");
    session
        .push_message(ConversationMessage::tool_result("t1", "bash", "ok", false))
        .expect("result");
    let view = reconcile(&session.messages, &known(&["bash", "read_file"]));
    assert!(
        std::sync::Arc::ptr_eq(&session.messages, &view),
        "all-known must keep the zero-copy Arc"
    );
}

#[test]
fn reconcile_rewrites_unknown_tool_use_and_result_to_text() {
    let mut session = Session::new();
    session.push_user_text("hi").expect("user");
    session
        .push_message(assistant_tool_use("t1", "mcp__gone__do"))
        .expect("assistant");
    session
        .push_message(ConversationMessage::tool_result(
            "t1",
            "mcp__gone__do",
            "the result",
            false,
        ))
        .expect("result");

    // The tool is genuinely gone → both blocks must become archived Text.
    let view = reconcile(&session.messages, &known(&["bash"]));
    assert!(
        !std::sync::Arc::ptr_eq(&session.messages, &view),
        "rewrite path must allocate a fresh Vec"
    );
    assert_eq!(
        count_blocks(&view, |b| matches!(b, ContentBlock::ToolUse { .. })),
        0,
        "no ToolUse for an unadvertised tool may survive"
    );
    assert_eq!(
        count_blocks(&view, |b| matches!(b, ContentBlock::ToolResult { .. })),
        0,
        "its paired ToolResult must be rewritten too (no orphan)"
    );
    let texts: String = view
        .iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        texts.contains("mcp__gone__do"),
        "names the dropped tool: {texts}"
    );
    assert!(
        texts.contains(
            "Archived tool activity: the removed tool \"mcp__gone__do\" was called earlier. \
             Recorded input: {}."
        ),
        "new wording must read as archived prose, not a callable line: {texts}"
    );
    assert!(
        texts.contains(
            "Archived tool activity: the removed tool \"mcp__gone__do\" returned earlier. \
             Recorded output: the result."
        ),
        "result wording: {texts}"
    );
    assert!(
        !texts.contains("— tool no longer available]"),
        "the old imitable fake-call syntax must be absent: {texts}"
    );
    assert!(
        texts.contains("the result"),
        "preserves the result text: {texts}"
    );
}

#[test]
fn reconcile_marks_unadvertised_but_resolvable_tools_as_capability_invocable() {
    let mut session = Session::new();
    session
        .push_message(assistant_tool_use("t1", "TaskList"))
        .expect("assistant");
    session
        .push_message(ConversationMessage::tool_result(
            "t1", "TaskList", "two tasks", false,
        ))
        .expect("result");

    let known_tools = known(&["bash", "CapabilityInvoke"]);
    let resolvable_tools = known(&["bash", "CapabilityInvoke", "TaskList"]);
    let view = reconcile_tool_history(&session.messages, &known_tools, &resolvable_tools);
    let texts = view
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        texts.contains(
            "Archived tool activity: the tool \"TaskList\" was called earlier. Recorded input: \
             {}. It remains callable through CapabilityInvoke."
        ),
        "call rewrite: {texts}"
    );
    assert!(
        texts.contains(
            "Archived tool activity: the tool \"TaskList\" returned earlier. Recorded output: \
             two tasks. It remains callable through CapabilityInvoke."
        ),
        "result rewrite: {texts}"
    );
    assert!(!texts.contains("no longer available"), "{texts}");
}

#[test]
fn reconcile_rewrites_orphan_tool_result_for_a_gone_tool() {
    // A tool_result naming a tool that is no longer advertised must be
    // rewritten even with NO paired tool_use, else it reaches the wire as an
    // unpaired tool_result and 400s the OpenAI-compatible path.
    let mut session = Session::new();
    session
        .push_message(ConversationMessage::tool_result(
            "ghost-id",
            "mcp__gone__do",
            "the result",
            false,
        ))
        .expect("orphan result append");

    let view = reconcile(&session.messages, &known(&["bash"]));
    assert!(
        !std::sync::Arc::ptr_eq(&session.messages, &view),
        "an orphan result for a gone tool must NOT hit the zero-copy fast path"
    );
    assert_eq!(
        count_blocks(&view, |b| matches!(b, ContentBlock::ToolResult { .. })),
        0,
        "the orphan tool_result must be rewritten to text"
    );
}

#[test]
fn reconcile_preserves_out_of_band_images_when_rewriting() {
    // A G10 image-bearing tool_result for a gone tool: the tool linkage is
    // dropped (no tool_result block) but the pixels must survive as Image.
    let mut session = Session::new();
    session
        .push_message(assistant_tool_use("img-1", "mcp__viz__shot"))
        .expect("assistant");
    session
        .push_message(ConversationMessage::tool_result_with_images(
            "img-1",
            "mcp__viz__shot",
            "captured",
            false,
            vec![("image/png".to_string(), "QUJD".to_string())],
        ))
        .expect("image result append");

    let view = reconcile(&session.messages, &known(&["bash"]));
    assert_eq!(
        count_blocks(&view, |b| matches!(b, ContentBlock::ToolResult { .. })),
        0,
        "tool_result linkage dropped"
    );
    let imgs: Vec<(&str, &str)> = view
        .iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|b| match b {
            ContentBlock::Image { media_type, data } => Some((media_type.as_str(), data.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        imgs,
        vec![("image/png", "QUJD")],
        "image preserved as a standalone block"
    );
}

#[test]
fn reconcile_keeps_known_tools_beside_unknown() {
    let mut session = Session::new();
    session
        .push_message(assistant_tool_use("keep", "bash"))
        .expect("assistant 1");
    session
        .push_message(ConversationMessage::tool_result(
            "keep", "bash", "ok", false,
        ))
        .expect("result 1");
    session
        .push_message(assistant_tool_use("drop", "ghost_tool"))
        .expect("assistant 2");
    session
        .push_message(ConversationMessage::tool_result(
            "drop",
            "ghost_tool",
            "x",
            false,
        ))
        .expect("result 2");

    let view = reconcile(&session.messages, &known(&["bash"]));
    // The known tool's blocks are untouched; only the ghost's are rewritten.
    assert_eq!(
        count_blocks(
            &view,
            |b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "bash")
        ),
        1,
        "known tool_use preserved"
    );
    assert_eq!(
        count_blocks(&view, |b| matches!(b, ContentBlock::ToolUse { .. })),
        1,
        "exactly one ToolUse remains (bash); ghost rewritten"
    );
    assert_eq!(
        count_blocks(
            &view,
            |b| matches!(b, ContentBlock::ToolResult { tool_name, .. } if tool_name == "bash")
        ),
        1,
        "known tool_result preserved"
    );
}

#[test]
fn reconcile_tool_history_is_a_pure_function_deterministic_on_the_rewrite_path() {
    // The zero-copy fast path is already pinned by
    // `reconcile_shares_arc_when_all_tools_known` (Arc::ptr_eq, trivially
    // equal). This covers the other half of the purity claim: the REWRITE
    // path (an unknown tool present) must still lower the same
    // (messages, known) pair to an equal `Vec<ConversationMessage>` every
    // call — no hidden ids, counters, or ordering nondeterminism.
    let mut session = Session::new();
    session.push_user_text("hi").expect("user");
    session
        .push_message(assistant_tool_use("t1", "mcp__gone__do"))
        .expect("assistant");
    session
        .push_message(ConversationMessage::tool_result(
            "t1",
            "mcp__gone__do",
            "the result",
            false,
        ))
        .expect("result");

    let known_tools = known(&["bash"]);
    let first = reconcile(&session.messages, &known_tools);
    let second = reconcile(&session.messages, &known_tools);
    assert_eq!(
        *first, *second,
        "same (messages, known) must reconcile to an equal result every call"
    );
}

#[test]
fn reconcile_tool_history_applied_twice_equals_applied_once() {
    // 2nd application == 1st application: once the unknown tool's blocks are
    // rewritten to Text/Image, reconciling that OUTPUT again against the same
    // known-set must be a true no-op — both structurally equal and, since no
    // unknown ids remain, back on the zero-copy Arc fast path.
    let mut session = Session::new();
    session
        .push_message(assistant_tool_use("t1", "mcp__gone__do"))
        .expect("assistant");
    session
        .push_message(ConversationMessage::tool_result(
            "t1",
            "mcp__gone__do",
            "the result",
            false,
        ))
        .expect("result");

    let known_tools = known(&["bash"]);
    let once = reconcile(&session.messages, &known_tools);
    let twice = reconcile(&once, &known_tools);
    assert_eq!(
        *once, *twice,
        "reconciling an already-reconciled view must be a no-op"
    );
    assert!(
        std::sync::Arc::ptr_eq(&once, &twice),
        "the second pass finds no unknown ids left -> zero-copy Arc"
    );
}

#[test]
fn tool_consistent_messages_seals_trailing_orphan() {
    let mut session = Session::new();
    session.push_user_text("hi").expect("user append");
    session
        .push_message(assistant_tool_use("tool-1", "bash"))
        .expect("assistant append");
    // No tool_result committed — turn cancelled mid-flight.
    let view = session.tool_consistent_messages();
    assert_eq!(view.len(), 3, "synthetic result appended");
    assert_eq!(view[2].role, MessageRole::Tool);
    match &view[2].blocks[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } => {
            assert_eq!(tool_use_id, "tool-1");
            assert!(is_error, "synthetic seal is an error result");
        }
        other => panic!("expected synthetic tool_result, got {other:?}"),
    }
    // Stored history is untouched (no mutation / re-persist).
    assert_eq!(session.messages.len(), 2);
}

/// r26 — the seal is a free function over a SLICE, because the compaction
/// summary request builds its body from `CompactionPlan` slices and never holds
/// a `Session` to ask. `tool_consistent_messages` is the whole-session wrapper
/// around this; there must be exactly one implementation of the rules
/// (synthetic `tool_result`, `is_error: true`, the interrupted wording), or the
/// two doors to the provider drift apart.
#[test]
fn seal_orphan_tool_uses_seals_a_bare_slice_the_way_the_session_view_does() {
    let messages = vec![
        ConversationMessage::user_text("run it"),
        assistant_tool_use("tool-1", "bash"),
        ConversationMessage::user_text("never mind"),
    ];
    let sealed = seal_orphan_tool_uses(&messages).expect("an orphan must be sealed");
    assert_eq!(sealed.len(), 4);
    match &sealed[2].blocks[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            output,
            ..
        } => {
            assert_eq!(tool_use_id, "tool-1");
            assert!(is_error, "synthetic seal is an error result");
            assert!(output.contains("Interrupted"), "{output}");
        }
        other => panic!("expected synthetic tool_result, got {other:?}"),
    }
    // The wrapper over a Session with the same messages agrees block for block.
    let mut session = Session::new();
    for message in &messages {
        session.push_message(message.clone()).expect("append");
    }
    assert_eq!(
        session.tool_consistent_messages().as_ref(),
        &sealed,
        "the session wrapper and the free function must be the same seal"
    );
}

/// Idempotent: a sealed view has no orphans left, so a second pass finds
/// nothing and says so (`None`) rather than stacking a second result behind the
/// same call — which would break pairing from the other side. The compaction
/// path can reach an already-sealed slice (a caller that seals a concatenation
/// and then re-seals a part of it), so this is a live property, not a
/// hypothetical.
#[test]
fn seal_orphan_tool_uses_twice_adds_one_result_not_two() {
    let messages = vec![
        ConversationMessage::user_text("run it"),
        assistant_tool_use("tool-1", "bash"),
    ];
    let once = seal_orphan_tool_uses(&messages).expect("first pass seals");
    assert_eq!(once.len(), 3);
    assert!(
        seal_orphan_tool_uses(&once).is_none(),
        "the second pass must find nothing to seal"
    );
}

/// `None` — not a copy — when every call is already answered, so the caller
/// keeps its `Arc` share or its borrow. This is what makes the seal free on the
/// happy path, which is every request of a healthy session.
#[test]
fn seal_orphan_tool_uses_returns_none_when_every_call_is_answered() {
    let messages = vec![
        ConversationMessage::user_text("run it"),
        assistant_tool_use("tool-1", "bash"),
        ConversationMessage::tool_result("tool-1", "bash", "ok", false),
    ];
    assert!(seal_orphan_tool_uses(&messages).is_none());
    assert!(
        seal_orphan_tool_uses(&[]).is_none(),
        "an empty slice has nothing to seal"
    );
}

/// The seam hazard the compaction caller exists to avoid: a `tool_use` closing
/// one slice and its result opening the next. Sealed apart, the first slice
/// reads as an orphan and gains a synthetic result the joined body then answers
/// twice. Sealed as one concatenation — what
/// `compaction_summary_request` does with `cache_prefix + messages_to_compact` —
/// there is nothing to seal.
#[test]
fn seal_orphan_tool_uses_over_a_concatenation_does_not_invent_a_seal_at_the_seam() {
    let head = vec![
        ConversationMessage::user_text("run it"),
        assistant_tool_use("tool-1", "bash"),
    ];
    let tail = [ConversationMessage::tool_result("tool-1", "bash", "ok", false)];

    assert!(
        seal_orphan_tool_uses(&head).is_some(),
        "premise: the head alone looks orphaned"
    );

    let joined: Vec<ConversationMessage> = head.iter().chain(tail.iter()).cloned().collect();
    assert!(
        seal_orphan_tool_uses(&joined).is_none(),
        "the concatenation is already well-formed and must be left alone"
    );
}

/// The synthetic results `seal_orphan_tool_uses` added, as `(position in the
/// sealed view, tool_use_id)`.
fn synthetic_seals(sealed: &[ConversationMessage]) -> Vec<(usize, String)> {
    sealed
        .iter()
        .enumerate()
        .flat_map(|(index, message)| {
            message.blocks.iter().filter_map(move |block| match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    output,
                    is_error: true,
                    ..
                } if output.contains("Interrupted") => Some((index, tool_use_id.clone())),
                _ => None,
            })
        })
        .collect()
}

/// Gemini mints its own call ids and reuses them across a conversation
/// (`call_18194` on 2026-09-11, 36 turns apart), and the wire gives the repeat
/// its own id. So an answer to the FIRST use says nothing about the second:
/// a repeat cancelled before its result is an orphan like any other, and left
/// unsealed it reaches the wire as a `functionCall` no `functionResponse`
/// follows — a 400 on every later request of an append-only history.
#[test]
fn a_reused_tool_id_answered_earlier_is_still_sealed_when_its_repeat_is_orphaned() {
    let messages = vec![
        ConversationMessage::user_text("look around"),
        assistant_tool_use("call_18194", "bash"),
        ConversationMessage::tool_result("call_18194", "bash", "ok", false),
        ConversationMessage::user_text("now read it"),
        assistant_tool_use("call_18194", "read_file"),
        // Ctrl+C before the read's result was written.
    ];
    let sealed = seal_orphan_tool_uses(&messages)
        .expect("the unanswered repeat must be sealed, not assumed answered");
    assert_eq!(synthetic_seals(&sealed), vec![(5, "call_18194".to_string())]);
    assert!(
        seal_orphan_tool_uses(&sealed).is_none(),
        "the sealed view pairs every occurrence and needs no second seal"
    );
}

/// The reverse order: an orphan sealed long ago must keep its seal when Gemini
/// later reuses its id for a call that IS answered — that answer belongs to
/// the later call, not the orphan.
#[test]
fn an_orphan_keeps_its_seal_when_a_later_call_reuses_its_id_and_is_answered() {
    let messages = vec![
        ConversationMessage::user_text("run it"),
        assistant_tool_use("call_1147386", "bash"),
        ConversationMessage::user_text("never mind"),
        assistant_tool_use("call_1147386", "read_file"),
        ConversationMessage::tool_result("call_1147386", "read_file", "body", false),
    ];
    let sealed = seal_orphan_tool_uses(&messages)
        .expect("the first use has no answer of its own and must be sealed");
    assert_eq!(synthetic_seals(&sealed), vec![(2, "call_1147386".to_string())]);
    assert_eq!(sealed[3].role, MessageRole::User, "the seal sits before the interleaved prompt");
    assert!(seal_orphan_tool_uses(&sealed).is_none());
}

/// Two calls with one id in ONE response, one answered before the cancel:
/// results answer that response's calls in order, so the second call is the
/// orphan and its seal lands after the first call's real result — the order
/// the wire pairs them in — never ahead of it.
#[test]
fn a_same_turn_duplicate_id_seals_the_unanswered_call_after_the_answered_one() {
    let messages = vec![
        ConversationMessage::user_text("read both"),
        ConversationMessage::assistant(vec![
            ContentBlock::ToolUse {
                id: "call_7".to_string(),
                name: "read_file".to_string(),
                input: r#"{"path":"a"}"#.to_string(),
            },
            ContentBlock::ToolUse {
                id: "call_7".to_string(),
                name: "read_file".to_string(),
                input: r#"{"path":"c"}"#.to_string(),
            },
        ]),
        ConversationMessage::tool_result("call_7", "read_file", "a body", false),
    ];
    let sealed = seal_orphan_tool_uses(&messages).expect("the second call is unanswered");
    assert_eq!(synthetic_seals(&sealed), vec![(3, "call_7".to_string())]);
    assert!(seal_orphan_tool_uses(&sealed).is_none());
}

#[test]
fn tool_consistent_messages_seals_orphan_before_following_user() {
    let mut session = Session::new();
    session.push_user_text("first").expect("user append");
    session
        .push_message(assistant_tool_use("tool-1", "SpawnMultiAgent"))
        .expect("assistant append");
    // Cancelled before the result; user sends another message — the exact
    // shape that produced `400 tool_use without tool_result`.
    session
        .push_user_text("are you working?")
        .expect("user append");
    let view = session.tool_consistent_messages();
    // user, assistant(tool_use), SYNTHETIC tool_result, user
    assert_eq!(view.len(), 4);
    assert!(matches!(view[2].blocks[0], ContentBlock::ToolResult { .. }));
    assert_eq!(view[3].role, MessageRole::User);
    assert_eq!(session.messages.len(), 3, "original session untouched");
}

#[test]
fn loads_legacy_session_json_object() {
    let path = temp_session_path("legacy");
    let legacy = JsonValue::Object(
        [
            ("version".to_string(), JsonValue::Number(1)),
            (
                "messages".to_string(),
                JsonValue::Array(vec![ConversationMessage::user_text("legacy").to_json()]),
            ),
        ]
        .into_iter()
        .collect(),
    );
    fs::write(&path, legacy.render()).expect("legacy file should write");

    let restored = Session::load_from_path(&path).expect("legacy session should load");
    fs::remove_file(&path).expect("temp file should be removable");

    assert_eq!(restored.messages.len(), 1);
    assert_eq!(
        restored.messages[0],
        ConversationMessage::user_text("legacy")
    );
    assert!(!restored.session_id.is_empty());
}

#[test]
fn appends_messages_to_persisted_jsonl_session() {
    let path = temp_session_path("append");
    let mut session = Session::new().with_persistence_path(path.clone());
    session
        .save_to_path(&path)
        .expect("initial save should succeed");
    session
        .push_user_text("hi")
        .expect("user append should succeed");
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "hello".to_string(),
        }]))
        .expect("assistant append should succeed");

    let restored = Session::load_from_path(&path).expect("session should replay from jsonl");
    fs::remove_file(&path).expect("temp file should be removable");

    assert_eq!(restored.messages.len(), 2);
    assert_eq!(restored.messages[0], ConversationMessage::user_text("hi"));
}

#[cfg(unix)]
#[test]
fn secure_session_persistence_rejects_symlink_load_and_append() {
    use std::os::unix::fs::symlink;

    let target = temp_session_path("secure-target");
    let link = temp_session_path("secure-link");
    let mut session = Session::new().with_secure_persistence_path(target.clone());
    session
        .push_user_text("kept")
        .expect("secure regular-file append should succeed");
    let restored = Session::load_from_secure_path(&target)
        .expect("secure regular-file load should succeed");
    assert_eq!(restored.messages, session.messages);

    symlink(&target, &link).expect("test symlink should be created");
    Session::load_from_secure_path(&link).expect_err("secure load must reject symlinks");

    let mut through_link = Session::new().with_secure_persistence_path(link.clone());
    through_link
        .push_user_text("blocked")
        .expect_err("secure append must reject symlinks");
    let unchanged = Session::load_from_secure_path(&target)
        .expect("symlink rejection must leave the target readable");
    assert_eq!(unchanged.messages, session.messages);

    fs::remove_file(&link).expect("test symlink should be removable");
    fs::remove_file(&target).expect("test target should be removable");
}

#[test]
fn persists_compaction_metadata() {
    let path = temp_session_path("compaction");
    let mut session = Session::new();
    session
        .push_user_text("before")
        .expect("message should append");
    session.record_compaction("summarized earlier work", 4);
    session.save_to_path(&path).expect("session should save");

    let restored = Session::load_from_path(&path).expect("session should load");
    fs::remove_file(&path).expect("temp file should be removable");

    let compaction = restored.compaction.expect("compaction metadata");
    assert_eq!(compaction.count, 1);
    assert_eq!(compaction.removed_message_count, 4);
    assert!(compaction.summary.contains("summarized"));
}

#[test]
fn record_compaction_rewrites_persisted_snapshot_to_compacted_state() {
    // Mirrors the production flow: messages are appended to disk as the
    // conversation grows, then compaction replaces the message vector and
    // calls record_compaction. The on-disk JSONL must reflect ONLY the
    // compacted messages, not the pre-compaction ones (no crash-window
    // divergence between memory and disk).
    let path = temp_session_path("compaction-rewrite");
    let mut session = Session::new().with_persistence_path(path.clone());
    session
        .push_user_text("first turn")
        .expect("user append should succeed");
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "first answer".to_string(),
        }]))
        .expect("assistant append should succeed");
    session
        .push_user_text("second turn")
        .expect("user append should succeed");

    // Simulate apply_compaction: replace the message vector with the
    // compacted set, then record the compaction (which re-persists).
    let compacted = vec![ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text {
            text: "Continuing from compacted context.".to_string(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    }];
    session.messages = std::sync::Arc::new(compacted.clone());
    session.record_compaction("summarized earlier work", 3);

    // Re-read from disk WITHOUT another explicit save: record_compaction
    // must have rewritten the snapshot.
    let restored = Session::load_from_path(&path).expect("session should load");
    fs::remove_file(&path).expect("temp file should be removable");

    assert_eq!(
        restored.messages.len(),
        1,
        "disk must hold only the compacted messages, not the appended originals"
    );
    assert_eq!(restored.messages[0], compacted[0]);
    let compaction = restored.compaction.expect("compaction metadata on disk");
    assert_eq!(compaction.count, 1);
    assert_eq!(compaction.removed_message_count, 3);
}

#[test]
fn seal_evicted_to_vault_preserves_raw_messages_losslessly() {
    let path = temp_session_path("vault-seal");
    let session = Session::new().with_persistence_path(path.clone());
    let evicted = vec![
        ConversationMessage::user_text("raw one"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "raw two".to_string(),
        }]),
    ];
    let _ = session.seal_evicted_to_vault(&evicted);

    let vault_path = vault_path_for(&path);
    let records = read_vault_records(&vault_path);
    let _ = fs::remove_file(&vault_path);

    assert_eq!(records.len(), 2, "every evicted message is sealed");
    assert_eq!(records[0].0, 0);
    assert_eq!(records[1].0, 1);
    // The vault holds the raw originals byte-for-byte, not a summary.
    assert_eq!(records[0].1, evicted[0].to_json());
    assert_eq!(records[1].1, evicted[1].to_json());
}

#[test]
fn vault_accumulates_across_compaction_rounds_without_overlapping_seqs() {
    let path = temp_session_path("vault-rounds");
    let mut session = Session::new().with_persistence_path(path.clone());

    // Round 1: first_message_index is 0, so this batch seals at seqs [0, 2).
    let round1 = vec![
        ConversationMessage::user_text("r1 a"),
        ConversationMessage::user_text("r1 b"),
    ];
    let _ = session.seal_evicted_to_vault(&round1);
    // Mirror production order: seal, then record_compaction (which advances
    // first_message_index by the removed count and re-persists the snapshot).
    session.messages = std::sync::Arc::new(vec![ConversationMessage::user_text("kept")]);
    session.record_compaction("summary one", round1.len());

    // Round 2: first_message_index is now 2, so this batch continues at [2, 3).
    let round2 = vec![ConversationMessage::user_text("r2 a")];
    let _ = session.seal_evicted_to_vault(&round2);

    let vault_path = vault_path_for(&path);
    let records = read_vault_records(&vault_path);
    let _ = fs::remove_file(&vault_path);
    let _ = fs::remove_file(&path);

    let seqs: Vec<u32> = records.iter().map(|(seq, _)| *seq).collect();
    assert_eq!(
        seqs,
        vec![0, 1, 2],
        "seqs are contiguous and never reused across rounds (round-1 raw survives round 2)"
    );
    assert_eq!(records[0].1, round1[0].to_json());
    assert_eq!(records[2].1, round2[0].to_json());
}

#[test]
fn read_vault_round_trips_sealed_messages() {
    let path = temp_session_path("vault-read");
    let session = Session::new().with_persistence_path(path.clone());
    let evicted = vec![
        ConversationMessage::user_text("first raw"),
        ConversationMessage::user_text("second raw"),
    ];
    let _ = session.seal_evicted_to_vault(&evicted);

    let records = session.read_vault();
    let _ = fs::remove_file(vault_path_for(&path));

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].vault_seq, 0);
    assert_eq!(records[1].vault_seq, 1);
    assert_eq!(records[0].message, evicted[0]);
    assert_eq!(records[1].message, evicted[1]);
}

#[test]
fn read_vault_deduplicates_reappended_seqs_last_wins() {
    // A crash between seal and the destructive rewrite (or two processes) can
    // re-seal the same seq range. The reader must dedup by seq, not double-count.
    let path = temp_session_path("vault-dedup");
    let session = Session::new().with_persistence_path(path.clone());
    let _ = session.seal_evicted_to_vault(&[ConversationMessage::user_text("v0")]);
    let _ = session.seal_evicted_to_vault(&[ConversationMessage::user_text("v0 again")]);

    let records = session.read_vault();
    let _ = fs::remove_file(vault_path_for(&path));

    assert_eq!(records.len(), 1, "duplicate seq collapses to one record");
    assert_eq!(records[0].vault_seq, 0);
    assert_eq!(
        records[0].message,
        ConversationMessage::user_text("v0 again"),
        "last write wins"
    );
}

#[test]
fn read_vault_skips_torn_or_corrupt_lines() {
    use std::io::Write;
    let path = temp_session_path("vault-torn");
    let session = Session::new().with_persistence_path(path.clone());
    let _ = session.seal_evicted_to_vault(&[ConversationMessage::user_text("good one")]);
    // Append a torn/garbage trailing line, as a crash mid-append would leave.
    let vault_path = vault_path_for(&path);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&vault_path)
        .expect("open vault");
    writeln!(file, "{{\"type\":\"vault\",\"vault_seq\":1,\"mess").expect("write torn line");
    drop(file);

    let records = session.read_vault();
    let _ = fs::remove_file(&vault_path);

    assert_eq!(records.len(), 1, "the torn line is skipped, the good record survives");
    assert_eq!(records[0].message, ConversationMessage::user_text("good one"));
}

#[test]
fn seal_evicted_to_vault_noops_without_persistence() {
    // An in-memory session (tests, `zo -p` without a store) has no path, so
    // sealing must be a silent no-op rather than a panic.
    let session = Session::new();
    let _ = session.seal_evicted_to_vault(&[ConversationMessage::user_text("ephemeral")]);
}

#[test]
fn anchor_summary_round_trips_through_session_jsonl() {
    let path = temp_session_path("anchor-roundtrip");
    let mut session = Session::new().with_persistence_path(path.clone());
    session.push_user_text("before").expect("append");
    session.messages = std::sync::Arc::new(vec![ConversationMessage::user_text("kept")]);
    let anchor = AnchorSummary {
        intent: "implement the vault".to_string(),
        concepts: vec!["Rust".to_string(), "append-only".to_string()],
        files: vec!["crates/core-types/src/session/mod.rs".to_string()],
        errors_and_fixes: vec!["E0277 resolved by impl Trait".to_string()],
        problem_solving: vec!["diagnosed summary-of-summary drift".to_string()],
        user_messages: vec!["please implement losslessly".to_string()],
        pending_tasks: vec!["wire the progress UI".to_string()],
        current_work: "writing serialization tests".to_string(),
        vault_ranges: vec![(0, 44), (45, 89)],
    };
    session.record_compaction_with_anchor("rendered summary", 1, Some(anchor.clone()));

    let restored = Session::load_from_path(&path).expect("session should load");
    let _ = fs::remove_file(&path);
    let restored_anchor = restored
        .compaction
        .and_then(|compaction| compaction.anchor)
        .expect("typed anchor should persist and reload");
    assert_eq!(restored_anchor, anchor, "every anchor section round-trips");
    assert_eq!(
        restored_anchor.vault_ranges,
        vec![(0, 44), (45, 89)],
        "the vault seq spans round-trip through the session JSONL"
    );
}

#[test]
fn seal_returns_span_and_rounds_do_not_overlap() {
    let path = temp_session_path("vault-span");
    let mut session = Session::new().with_persistence_path(path.clone());

    // Round 1: first_message_index is 0, three messages seal at seqs [0, 2].
    let round1 = vec![
        ConversationMessage::user_text("a"),
        ConversationMessage::user_text("b"),
        ConversationMessage::user_text("c"),
    ];
    let span1 = session
        .seal_evicted_to_vault(&round1)
        .expect("round 1 seals a span");
    assert_eq!(span1, (0, 2), "the span covers exactly the sealed seqs");

    // Advance first_message_index as record_compaction does before round 2.
    session.messages = std::sync::Arc::new(vec![ConversationMessage::user_text("kept")]);
    session.record_compaction("summary one", round1.len());

    // Round 2: first_message_index is now 3, two messages seal at seqs [3, 4].
    let round2 = vec![
        ConversationMessage::user_text("d"),
        ConversationMessage::user_text("e"),
    ];
    let span2 = session
        .seal_evicted_to_vault(&round2)
        .expect("round 2 seals a span");
    let _ = fs::remove_file(vault_path_for(&path));
    let _ = fs::remove_file(&path);

    assert_eq!(span2, (3, 4));
    assert!(
        span1.1 < span2.0,
        "consecutive rounds' spans never overlap ({span1:?} then {span2:?})"
    );
}

#[test]
fn seal_returns_none_without_persistence_or_empty_batch() {
    // No persistence path (in-memory session) → no seal, no span.
    let session = Session::new();
    assert_eq!(
        session.seal_evicted_to_vault(&[ConversationMessage::user_text("x")]),
        None,
        "unpersisted session cannot seal"
    );
    // Empty batch → nothing to seal.
    let path = temp_session_path("vault-empty");
    let session = Session::new().with_persistence_path(path);
    assert_eq!(session.seal_evicted_to_vault(&[]), None, "empty batch seals nothing");
}

#[test]
fn anchor_without_vault_ranges_loads_as_empty() {
    // A record whose anchor predates the vault_ranges field (the key is simply
    // absent) must load with an empty ranges vec, not error.
    let path = temp_session_path("anchor-no-ranges");
    let mut session = Session::new().with_persistence_path(path.clone());
    session.push_user_text("before").expect("append");
    session.messages = std::sync::Arc::new(vec![ConversationMessage::user_text("kept")]);
    let anchor = AnchorSummary {
        intent: "older anchor with no ranges".to_string(),
        ..AnchorSummary::default()
    };
    // Serialization omits an empty `vault_ranges`, so the persisted record has no
    // such key — exactly the shape a pre-P1-ranges binary wrote.
    session.record_compaction_with_anchor("summary", 1, Some(anchor));

    let contents = fs::read_to_string(&path).expect("read session");
    assert!(
        !contents.contains("vault_ranges"),
        "an empty ranges vec is not serialized: {contents}"
    );
    let restored = Session::load_from_path(&path).expect("session should load");
    let _ = fs::remove_file(&path);
    let restored_anchor = restored
        .compaction
        .and_then(|compaction| compaction.anchor)
        .expect("anchor reloads");
    assert!(
        restored_anchor.vault_ranges.is_empty(),
        "absent vault_ranges loads as empty"
    );
    assert_eq!(restored_anchor.intent, "older anchor with no ranges");
}

#[test]
fn compaction_record_tolerates_unknown_forward_compatible_keys() {
    // Later LAVA phases add new optional fields (anchor/vault_range/...) to the
    // compaction record. An older binary must still load such a record by
    // ignoring unknown keys — this guards the decision that additive
    // compaction-record fields need no schema-version gate.
    let path = temp_session_path("compaction-unknown-keys");
    let mut session = Session::new().with_persistence_path(path.clone());
    session.push_user_text("before").expect("append");
    session.messages = std::sync::Arc::new(vec![ConversationMessage::user_text("kept")]);
    session.record_compaction("summary", 1);

    let contents = fs::read_to_string(&path).expect("read session");
    let patched = contents
        .lines()
        .map(|line| {
            if line.contains("\"type\":\"compaction\"") {
                line.replacen('}', r#","anchor":{"intent":"future"},"vault_range":[0,1]}"#, 1)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&path, format!("{patched}\n")).expect("write patched session");

    let restored =
        Session::load_from_path(&path).expect("unknown compaction keys must not brick load");
    let _ = fs::remove_file(&path);
    let compaction = restored
        .compaction
        .expect("compaction metadata survives unknown keys");
    assert_eq!(compaction.count, 1);
    assert_eq!(compaction.removed_message_count, 1);
}

#[test]
fn forks_sessions_with_branch_metadata_and_persists_it() {
    let path = temp_session_path("fork");
    let mut session = Session::new();
    session
        .push_user_text("before fork")
        .expect("message should append");

    let forked = session
        .fork(Some("investigation".to_string()))
        .with_persistence_path(path.clone());
    forked
        .save_to_path(&path)
        .expect("forked session should save");

    let restored = Session::load_from_path(&path).expect("forked session should load");
    fs::remove_file(&path).expect("temp file should be removable");

    assert_ne!(restored.session_id, session.session_id);
    assert_eq!(
        restored.fork,
        Some(SessionFork {
            parent_session_id: session.session_id,
            branch_name: Some("investigation".to_string()),
        })
    );
    assert_eq!(restored.messages, forked.messages);
}

#[test]
fn rotates_and_cleans_up_large_session_logs() {
    // given
    let path = temp_session_path("rotation");
    let oversized_length =
        usize::try_from(super::ROTATE_AFTER_BYTES + 10).expect("rotate threshold should fit");
    fs::write(&path, "x".repeat(oversized_length)).expect("oversized file should write");

    // when — a snapshot that sheds the whole transcript (a compaction) is what
    // the backup exists for.
    assert!(
        rotate_session_file_if_needed(&path, 0).expect("rotation should succeed"),
        "a shedding snapshot rotates"
    );

    // then
    assert!(
        !path.exists(),
        "original path should be rotated away before rewrite"
    );

    for _ in 0..5 {
        let rotated = super::rotated_log_path(&path);
        fs::write(&rotated, "old").expect("rotated file should write");
    }
    cleanup_rotated_logs(&path).expect("cleanup should succeed");

    let rotated_count = rotation_files(&path).len();
    assert!(rotated_count <= super::MAX_ROTATED_FILES);
    for rotated in rotation_files(&path) {
        fs::remove_file(rotated).expect("rotated file should be removable");
    }
}

#[test]
fn ordinary_turn_persistence_does_not_rotate_oversized_session() {
    let path = temp_session_path("ordinary-turn-no-rotation");
    let oversized_length =
        usize::try_from(super::ROTATE_AFTER_BYTES + 10).expect("rotate threshold should fit");
    let mut session = Session::new().with_persistence_path(path.clone());
    session
        .push_user_text("x".repeat(oversized_length))
        .expect("oversized message should append");

    for turn in 0..3 {
        session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: format!("ordinary turn {turn}"),
            }]))
            .expect("ordinary message should append");
        session
            .persist_appended_state_to_path(&path)
            .expect("ordinary turn should persist");
    }

    let rotations = rotation_files(&path);
    assert!(
        rotations.is_empty(),
        "ordinary turns must not rotate/rewrite an append-durable transcript, got {rotations:?}"
    );
    let _ = fs::remove_file(&path);
    for rotated in rotations {
        let _ = fs::remove_file(rotated);
    }
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

#[test]
fn append_aware_persist_rejects_a_stale_store() {
    let path = temp_session_path("append-aware-stale-store");
    let mut session = Session::new().with_persistence_path(path.clone());
    session
        .push_user_text("durable before peer rewrite")
        .expect("initial append should persist");
    fs::write(&path, "peer replaced this session\n").expect("simulate peer rewrite");

    let error = session
        .persist_appended_state_to_path(&path)
        .expect_err("stale append-aware persistence must fail");
    assert!(matches!(error, SessionError::Conflict(_)));

    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

#[test]
fn appended_turn_reload_matches_forced_full_snapshot() {
    let path = temp_session_path("appended-turn-load-parity");
    let mut session = Session::new().with_persistence_path(path.clone());
    session.name = Some("load parity".to_string());
    session.session_goal = Some("preserve append durability".to_string());
    session.save_to_path(&path).expect("seed full snapshot");
    while super::current_time_millis() <= session.updated_at_ms {
        std::thread::yield_now();
    }

    session.push_user_text("first ordinary turn").expect("append user");
    let mut assistant = ConversationMessage::assistant_with_usage(
        vec![ContentBlock::Text {
            text: "first answer".to_string(),
        }],
        Some(TokenUsage {
            input_tokens: 10,
            output_tokens: 4,
            cache_creation_input_tokens: 1,
            cache_read_input_tokens: 2,
            output_tokens_details: None,
        }),
    );
    assistant.model = Some("test-model".to_string());
    session.push_message(assistant).expect("append assistant");
    let appended_bytes = fs::read(&path).expect("read append-only session");
    session
        .persist_appended_state_to_path(&path)
        .expect("clean ordinary turn should persist without rewrite");
    assert_eq!(
        fs::read(&path).expect("read clean persisted session"),
        appended_bytes,
        "clean ordinary persistence must leave the append stream byte-for-byte unchanged"
    );

    session.name = Some("renamed parity session".to_string());
    session.session_goal = Some("updated metadata survives".to_string());
    session
        .persist_appended_state_to_path(&path)
        .expect("dirty metadata should force a snapshot");
    session
        .push_user_text("second ordinary turn")
        .expect("append second user");
    session
        .persist_appended_state_to_path(&path)
        .expect("second clean ordinary turn should not rewrite");
    if let Some(last) = std::sync::Arc::make_mut(&mut session.messages).last_mut() {
        last.blocks.push(ContentBlock::Text {
            text: "folded steering".to_string(),
        });
    }
    session.mark_transcript_dirty();
    session
        .persist_appended_state_to_path(&path)
        .expect("in-place transcript mutation should force a snapshot");
    session
        .push_user_text("third ordinary turn after healing snapshot")
        .expect("append third user");
    session
        .persist_appended_state_to_path(&path)
        .expect("clean turn after healing snapshot should not rewrite");

    let appended_reload = Session::load_from_path(&path).expect("load append path");
    session.save_to_path(&path).expect("force full snapshot");
    let snapshot_reload = Session::load_from_path(&path).expect("load forced snapshot");
    assert_eq!(
        appended_reload, snapshot_reload,
        "append-aware persistence and a forced full snapshot must reload identically"
    );
    assert_eq!(appended_reload.messages, session.messages);
    assert_eq!(appended_reload.name, session.name);
    assert_eq!(appended_reload.session_goal, session.session_goal);
    assert_eq!(appended_reload.updated_at_ms, session.updated_at_ms);

    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

#[test]
fn rejects_jsonl_record_without_type() {
    // given
    let path = write_temp_session_file(
        "missing-type",
        r#"{"message":{"role":"user","blocks":[{"type":"text","text":"hello"}]}}"#,
    );

    // when
    let error = Session::load_from_path(&path)
        .expect_err("session should reject JSONL records without a type");

    // then
    assert!(error.to_string().contains("missing type"));
    fs::remove_file(path).expect("temp file should be removable");
}

#[test]
fn rejects_jsonl_message_record_without_message_payload() {
    // given
    let path = write_temp_session_file("missing-message", r#"{"type":"message"}"#);

    // when
    let error = Session::load_from_path(&path)
        .expect_err("session should reject JSONL message records without message payload");

    // then
    assert!(error.to_string().contains("missing message"));
    fs::remove_file(path).expect("temp file should be removable");
}

#[test]
fn rejects_jsonl_record_with_unknown_type() {
    // given
    let path = write_temp_session_file("unknown-type", r#"{"type":"mystery"}"#);

    // when
    let error = Session::load_from_path(&path)
        .expect_err("session should reject unknown JSONL record types");

    // then
    assert!(error.to_string().contains("unsupported JSONL record type"));
    fs::remove_file(path).expect("temp file should be removable");
}

#[test]
fn rejects_legacy_session_json_without_messages() {
    // given
    let session = JsonValue::Object(
        [("version".to_string(), JsonValue::Number(1))]
            .into_iter()
            .collect(),
    );

    // when
    let error =
        Session::from_json(&session).expect_err("legacy session objects should require messages");

    // then
    assert!(error.to_string().contains("missing messages"));
}

#[test]
fn normalizes_blank_fork_branch_name_to_none() {
    // given
    let session = Session::new();

    // when
    let forked = session.fork(Some("   ".to_string()));

    // then
    assert_eq!(forked.fork.expect("fork metadata").branch_name, None);
}

#[test]
fn rejects_unknown_content_block_type() {
    // given
    let block = JsonValue::Object(
        [("type".to_string(), JsonValue::String("unknown".to_string()))]
            .into_iter()
            .collect(),
    );

    // when
    let error =
        ContentBlock::from_json(&block).expect_err("content blocks should reject unknown types");

    // then
    assert!(error.to_string().contains("unsupported block type"));
}

fn temp_session_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("runtime-session-{label}-{nanos}.json"))
}

fn write_temp_session_file(label: &str, contents: &str) -> PathBuf {
    let path = temp_session_path(label);
    fs::write(&path, format!("{contents}\n")).expect("temp session file should write");
    path
}

fn rotation_files(path: &Path) -> Vec<PathBuf> {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("temp path should have file stem")
        .to_string();
    fs::read_dir(path.parent().expect("temp path should have parent"))
        .expect("temp dir should read")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|entry_path| {
            entry_path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("{stem}.rot-"))
                        && Path::new(name)
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
                })
        })
        .collect()
}

/// Read the Raw Vault sidecar back into `(vault_seq, message_json)` pairs,
/// asserting every record is a well-formed `vault` record.
fn read_vault_records(path: &Path) -> Vec<(u32, JsonValue)> {
    let contents = fs::read_to_string(path).expect("vault file should exist after sealing");
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value = JsonValue::parse(line.trim()).expect("vault line should be valid json");
            let object = value.as_object().expect("vault record should be an object");
            assert_eq!(
                object.get("type").and_then(JsonValue::as_str),
                Some("vault"),
                "every vault record carries type=vault"
            );
            let seq = object
                .get("vault_seq")
                .and_then(JsonValue::as_i64)
                .and_then(|raw| u32::try_from(raw).ok())
                .expect("vault_seq should be present and in range");
            let message = object
                .get("message")
                .cloned()
                .expect("vault record should carry the raw message");
            (seq, message)
        })
        .collect()
}

// ── rewind_turns tests ──────────────────────────────────────────

fn make_tool_result_message(tool_use_id: &str) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            tool_name: "bash".to_string(),
            output: "ok".to_string(),
            is_error: false,
            images: Vec::new(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    }
}

#[test]
fn rewind_simple_turn() {
    let mut session = Session::new();
    session.push_user_text("hi").unwrap();
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "hello".into(),
        }]))
        .unwrap();
    assert_eq!(session.messages.len(), 2);

    let removed = session.rewind_turns(1);
    assert_eq!(removed, 2); // user + assistant
    assert!(session.messages.is_empty());
}

#[test]
fn rewind_turn_with_tool_results() {
    let mut session = Session::new();
    session.push_user_text("do something").unwrap();
    session
        .push_message(ConversationMessage::assistant(vec![
            ContentBlock::Text {
                text: "let me run that".into(),
            },
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: "echo hi".into(),
            },
        ]))
        .unwrap();
    session
        .push_message(make_tool_result_message("t1"))
        .unwrap();
    assert_eq!(session.messages.len(), 3);

    let removed = session.rewind_turns(1);
    assert_eq!(removed, 3); // tool_result + assistant + user
    assert!(session.messages.is_empty());
}

#[test]
fn rewind_multiple_turns() {
    let mut session = Session::new();
    for i in 0..3 {
        session.push_user_text(format!("q{i}")).unwrap();
        session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: format!("a{i}"),
            }]))
            .unwrap();
    }
    assert_eq!(session.messages.len(), 6);

    let removed = session.rewind_turns(2);
    assert_eq!(removed, 4); // 2 turns × 2 messages
    assert_eq!(session.messages.len(), 2);
}

#[test]
fn rewind_zero_is_noop() {
    let mut session = Session::new();
    session.push_user_text("hi").unwrap();
    let removed = session.rewind_turns(0);
    assert_eq!(removed, 0);
    assert_eq!(session.messages.len(), 1);
}

#[test]
fn rewind_more_than_available() {
    let mut session = Session::new();
    session.push_user_text("hi").unwrap();
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "hello".into(),
        }]))
        .unwrap();
    let removed = session.rewind_turns(10);
    assert_eq!(removed, 2);
    assert!(session.messages.is_empty());
}

/// A reminder microcompact cleared is still the annotation it always was.
/// Rewind walks backward over reminder annotations because they belong to the
/// turn they annotate; a cleared block that stopped answering to
/// `is_reminder_annotation` would halt that walk on a message the turn owns —
/// burning a `turns_removed` step that removes nothing and stranding the
/// placeholder above the prompt it annotated.
#[test]
fn rewind_treats_a_cleared_reminder_as_the_annotation_it_replaced() {
    let cleared = ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text {
            text: super::CLEARED_REMINDER_PLACEHOLDER.to_string(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    };
    assert!(
        cleared.is_reminder_annotation(),
        "clearing changes the bytes, not what the message is"
    );

    let mut session = Session::new();
    session.push_user_text("hi").unwrap();
    session.push_message(cleared).unwrap();
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "hello".into(),
        }]))
        .unwrap();

    let removed = session.rewind_turns(1);
    assert_eq!(removed, 3, "assistant + the annotation + the prompt");
    assert!(session.messages.is_empty());
}

/// /goal 영속: 세션 헤더에 저장돼 JSONL/JSON 양쪽 라운드트립으로 복원되고,
/// goal 없는 옛 파일은 그대로 None으로 로드된다(후방호환).
#[test]
fn session_goal_round_trips_and_old_files_stay_compatible() {
    let path = temp_session_path("goal-roundtrip");
    let mut session = Session::new();
    session.session_goal = Some("ship the refactor".to_string());
    session.save_to_path(&path).expect("save");
    let loaded = Session::load_from_path(&path).expect("load");
    assert_eq!(loaded.session_goal.as_deref(), Some("ship the refactor"));
    // fork도 goal을 승계한다.
    assert_eq!(
        loaded.fork(None).session_goal.as_deref(),
        Some("ship the refactor")
    );
    // goal 필드가 없는 종전 스냅샷은 None으로 로드.
    let mut bare = Session::new();
    bare.session_goal = None;
    bare.save_to_path(&path).expect("save bare");
    let loaded = Session::load_from_path(&path).expect("load bare");
    assert!(loaded.session_goal.is_none());
    let _ = fs::remove_file(&path);
}

#[test]
fn session_name_round_trips_and_legacy_files_remain_unnamed() {
    let path = temp_session_path("name-roundtrip");
    let mut session = Session::new();
    session.name = Some("배포 관찰".to_string());

    let json = session.to_json().expect("serialize legacy JSON shape");
    assert_eq!(
        Session::from_json(&json)
            .expect("deserialize legacy JSON shape")
            .name
            .as_deref(),
        Some("배포 관찰")
    );

    session.save_to_path(&path).expect("save named session");
    let loaded = Session::load_from_path(&path).expect("load named session");
    assert_eq!(loaded.name.as_deref(), Some("배포 관찰"));
    assert_eq!(loaded.fork(None).name.as_deref(), Some("배포 관찰"));

    fs::write(
        &path,
        "{\"type\":\"session_meta\",\"version\":1,\"session_id\":\"legacy\",\"created_at_ms\":1,\"updated_at_ms\":1}\n",
    )
    .expect("write legacy session");
    let legacy = Session::load_from_path(&path).expect("load legacy session without name");
    assert!(legacy.name.is_none());
    let _ = fs::remove_file(&path);
}

#[cfg(unix)]
#[test]
fn secure_persistence_rejects_symlinks_and_hardlinks() {
    use std::os::unix::fs::symlink;

    let path = temp_session_path("secure-links");
    let victim = path.with_extension("victim");
    fs::write(&victim, "do not touch").expect("write victim");

    symlink(&victim, &path).expect("create transcript symlink");
    assert!(Session::load_from_secure_path(&path).is_err());
    let mut session = Session::new().with_secure_persistence_path(path.clone());
    assert!(session.push_user_text("must not follow").is_err());
    assert_eq!(fs::read_to_string(&victim).expect("read victim"), "do not touch");

    fs::remove_file(&path).expect("remove symlink");
    fs::hard_link(&victim, &path).expect("create transcript hardlink");
    assert!(Session::load_from_secure_path(&path).is_err());
    let mut session = Session::new().with_secure_persistence_path(path.clone());
    assert!(session.push_user_text("must not append").is_err());
    assert_eq!(fs::read_to_string(&victim).expect("read victim"), "do not touch");

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(victim);
}

// --- atomic-write temp-file collision safety -------------------------------

/// Every distinct temp name a writer draws must be unique: successive draws
/// (the retry sequence) never repeat, and a draw stays in the target's own
/// directory so the publishing rename remains same-filesystem/atomic.
#[test]
fn temporary_paths_are_unique_and_same_directory() {
    let base = temp_session_path("temp-unique");
    let parent = base.parent().expect("temp path has parent");
    let mut seen = std::collections::HashSet::new();
    for attempt in 0..64u32 {
        let candidate = super::temporary_path_for(&base, attempt);
        assert_eq!(
            candidate.parent(),
            Some(parent),
            "temp file must stay in the target directory for an atomic rename"
        );
        assert!(
            seen.insert(candidate.clone()),
            "temp name repeated: {}",
            candidate.display()
        );
    }
}

/// Two writers hammering the *same* session path concurrently must never
/// truncate or steal each other's temp file. Each thread writes its own full
/// payload many times; after the storm the published file must be exactly one
/// writer's payload verbatim — never a torn mixture, never empty — and no temp
/// remnant may survive.
#[test]
fn concurrent_write_atomic_never_corrupts_or_steals_temp() {
    let path = temp_session_path("concurrent-atomic");
    let _ = fs::remove_file(&path);
    let payload_a = "AAAA-payload-from-writer-a".repeat(64);
    let payload_b = "BBBB-payload-from-writer-b".repeat(64);

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for payload in [payload_a.clone(), payload_b.clone()] {
        let path = path.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        let expected_a = payload_a.clone();
        let expected_b = payload_b.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..200 {
                super::write_atomic(&path, &payload).expect("atomic write should succeed");
                // Whatever is published mid-storm must be a *complete* payload,
                // never a partial/torn file from a stolen temp.
                let published = fs::read_to_string(&path).expect("read published");
                assert!(
                    published == expected_a || published == expected_b,
                    "published file must be exactly one writer's payload, got {} bytes",
                    published.len()
                );
            }
        }));
    }
    for handle in handles {
        handle.join().expect("writer thread should not panic");
    }

    let final_contents = fs::read_to_string(&path).expect("read final");
    assert!(final_contents == payload_a || final_contents == payload_b);

    let remnants: Vec<_> = fs::read_dir(path.parent().unwrap())
        .expect("read dir")
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("concurrent-atomic") && name.contains(".tmp-")
        })
        .collect();
    assert!(
        remnants.is_empty(),
        "no temp remnants should survive: {remnants:?}"
    );
    let _ = fs::remove_file(&path);
}

/// A stale temp left by a crashed process (the exact name a fresh writer would
/// draw first) must not wedge or corrupt the next save: the exclusive-create
/// retry simply picks another name and publishes cleanly.
#[test]
fn stale_crash_temp_does_not_block_next_write() {
    let path = temp_session_path("stale-temp");
    let _ = fs::remove_file(&path);
    // Pre-create the exact first-attempt temp name a writer will try, mimicking
    // a crash between temp creation and rename. It must survive (we don't own
    // it) yet not stop the real write from succeeding under a fresh name.
    let stale = super::temporary_path_for(&path, 0);
    fs::write(&stale, "stale-bytes-from-crashed-writer").expect("plant stale temp");

    super::write_atomic(&path, "fresh-payload").expect("write must succeed past stale temp");
    assert_eq!(
        fs::read_to_string(&path).expect("read published"),
        "fresh-payload"
    );
    let _ = fs::remove_file(&stale);
    let _ = fs::remove_file(&path);
}

/// The secure path must share the same collision safety: two secure writers on
/// one path never fail spuriously with `AlreadyExists` nor corrupt the file.
#[cfg(unix)]
#[test]
fn concurrent_write_atomic_secure_never_fails_spuriously() {
    let path = temp_session_path("concurrent-secure");
    let _ = fs::remove_file(&path);
    let payload_a = "secure-a".repeat(64);
    let payload_b = "secure-b".repeat(64);

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for payload in [payload_a.clone(), payload_b.clone()] {
        let path = path.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        let expected_a = payload_a.clone();
        let expected_b = payload_b.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..200 {
                super::write_atomic_secure(&path, &payload)
                    .expect("secure atomic write must not fail spuriously on collision");
                let published = fs::read_to_string(&path).expect("read published");
                assert!(published == expected_a || published == expected_b);
            }
        }));
    }
    for handle in handles {
        handle.join().expect("secure writer thread should not panic");
    }
    let _ = fs::remove_file(&path);
}

// --- writer lease + stale-snapshot conflict guard --------------------------

fn persisted_session(path: &std::path::Path) -> Session {
    Session::new().with_persistence_path(path.to_path_buf())
}

fn push_text(session: &mut Session, text: &str) -> Result<(), super::SessionError> {
    session.push_message(ConversationMessage::user_text(text))
}

fn is_conflict(result: &Result<(), super::SessionError>) -> bool {
    matches!(result, Err(super::SessionError::Conflict(_)))
}

/// Today's home-migration incident, pinned end-to-end: a session LOADED from
/// disk that keeps writing, whose store directory then disappears (a root
/// move), must refuse the next write with the fingerprint guard's `Conflict`
/// — and must NOT silently recreate the store at the stale path. The future
/// recovery seam (re-resolving the session id in the current root chain)
/// builds on this refusal staying loud and classified.
#[test]
fn moved_store_directory_refuses_next_write_with_conflict() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    let store = std::env::temp_dir().join(format!("runtime-session-moved-store-{nanos}"));
    fs::create_dir_all(&store).expect("create isolated store dir");
    let path = store.join("session.jsonl");
    {
        let mut writer = persisted_session(&path);
        push_text(&mut writer, "before-load").expect("seed the on-disk session");
    }

    let mut reloaded = Session::load_from_path(&path).expect("load the persisted session");
    push_text(&mut reloaded, "after-load").expect("loaded session keeps writing in place");

    let aside = std::env::temp_dir().join(format!("runtime-session-moved-store-{nanos}-aside"));
    fs::rename(&store, &aside).expect("move the store dir aside");

    let result = push_text(&mut reloaded, "after-move");
    assert!(
        is_conflict(&result),
        "write into a moved-away store must refuse with the fingerprint Conflict, got {result:?}"
    );
    assert!(
        !path.exists(),
        "a failed write must not silently recreate the stale store path"
    );

    fs::remove_dir_all(&aside).ok();
}

/// Candidate derivation is strictly shape-bound: only a canonical
/// `projects/<slug>/sessions/<file>` tail re-roots, so ad-hoc stores never
/// rebind implicitly.
#[test]
fn moved_store_candidates_require_canonical_shape() {
    let bound = Path::new("/old-root/projects/my-proj/sessions/session-1.jsonl");
    let roots = vec![PathBuf::from("/new-root"), PathBuf::from("/other")];
    assert_eq!(
        Session::moved_store_candidates(bound, &roots),
        vec![
            PathBuf::from("/new-root/projects/my-proj/sessions/session-1.jsonl"),
            PathBuf::from("/other/projects/my-proj/sessions/session-1.jsonl"),
        ]
    );
    assert!(
        Session::moved_store_candidates(Path::new("/tmp/ad-hoc/session.jsonl"), &roots).is_empty(),
        "non-canonical stores must never produce rebind candidates"
    );
}

/// The recovery seam for today's incident: when the WHOLE root moves and the
/// relocated transcript (same session id) exists under a current root, the
/// session rebinds once and keeps writing at the new location — the stale
/// path is not recreated and the fingerprint guard re-arms on the new path.
#[test]
fn moved_store_rebinds_to_relocated_root_and_keeps_writing() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    let root_a = std::env::temp_dir().join(format!("runtime-session-rebind-{nanos}-a"));
    let sessions_a = root_a.join("projects").join("proj").join("sessions");
    fs::create_dir_all(&sessions_a).expect("create canonical store");
    let bound = sessions_a.join("session-rebind.jsonl");

    let mut session = persisted_session(&bound);
    push_text(&mut session, "before-move").expect("seed the on-disk session");

    let root_b = std::env::temp_dir().join(format!("runtime-session-rebind-{nanos}-b"));
    fs::rename(&root_a, &root_b).expect("relocate the whole root");
    assert!(!bound.exists());

    assert!(
        session.rebind_moved_store_with_roots(std::slice::from_ref(&root_b)),
        "identity-matching relocated transcript must rebind"
    );
    push_text(&mut session, "after-rebind").expect("write continues at the relocated store");

    let relocated = root_b
        .join("projects")
        .join("proj")
        .join("sessions")
        .join("session-rebind.jsonl");
    let contents = fs::read_to_string(&relocated).expect("read relocated transcript");
    assert!(contents.contains("after-rebind"), "{contents}");
    assert!(!bound.exists(), "stale path must not be recreated");

    fs::remove_dir_all(&root_b).ok();
}

#[test]
fn stale_same_id_copy_is_never_adopted() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    let root_a = std::env::temp_dir().join(format!("runtime-session-stale-rebind-{nanos}-a"));
    let sessions_a = root_a.join("projects").join("proj").join("sessions");
    fs::create_dir_all(&sessions_a).expect("create canonical store");
    let bound = sessions_a.join("session-rebind.jsonl");

    let mut session = persisted_session(&bound);
    push_text(&mut session, "stale-state").expect("seed the on-disk session");
    let stale = fs::read(&bound).expect("snapshot stale transcript");
    push_text(&mut session, "newer-state").expect("advance the live transcript");
    assert_ne!(
        fs::read(&bound).expect("read advanced transcript"),
        stale,
        "the stale candidate must differ from the writer's last observation"
    );

    let aside = std::env::temp_dir().join(format!("runtime-session-stale-rebind-{nanos}-aside"));
    fs::rename(&root_a, &aside).expect("move the live store aside");

    let root_b = std::env::temp_dir().join(format!("runtime-session-stale-rebind-{nanos}-b"));
    let candidate = root_b
        .join("projects")
        .join("proj")
        .join("sessions")
        .join("session-rebind.jsonl");
    fs::create_dir_all(candidate.parent().expect("candidate parent"))
        .expect("create candidate store");
    fs::write(&candidate, &stale).expect("publish stale same-id candidate");

    assert!(
        !session.rebind_moved_store_with_roots(std::slice::from_ref(&root_b)),
        "same session id is insufficient when candidate bytes are stale"
    );
    let result = push_text(&mut session, "must-not-append");
    assert!(
        is_conflict(&result),
        "a stale candidate must leave the writer on the missing path and conflict, got {result:?}"
    );
    assert_eq!(
        fs::read(&candidate).expect("read rejected candidate"),
        stale,
        "a rejected stale candidate must never be appended to"
    );

    fs::remove_dir_all(&aside).ok();
    fs::remove_dir_all(&root_b).ok();
}

/// Two independently loaded sessions on the same path: while writer A is alive
/// and holds the lease, B's write is refused with a Conflict (not a silent
/// clobber, not a block).
#[test]
fn live_second_writer_is_refused_while_first_alive() {
    let path = temp_session_path("lease-live");
    let _ = fs::remove_file(&path);

    let mut a = persisted_session(&path);
    push_text(&mut a, "a-first").expect("A first push acquires lease and writes");

    let mut b = persisted_session(&path);
    let b_result = push_text(&mut b, "b-first");
    assert!(
        is_conflict(&b_result),
        "second live writer must be refused with Conflict, got {b_result:?}"
    );

    // A keeps working.
    push_text(&mut a, "a-second").expect("A continues writing under its lease");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// After writer A drops (lease released), a *stale* B that loaded the same file
/// before A's later writes must still be refused: its expected fingerprint no
/// longer matches disk, so a full-snapshot save cannot clobber A's newer state.
#[test]
fn stale_writer_after_drop_is_refused_by_fingerprint() {
    let path = temp_session_path("lease-stale");
    let _ = fs::remove_file(&path);

    // A writes an initial state.
    let mut a = persisted_session(&path);
    push_text(&mut a, "a-1").expect("A writes initial");

    // B loads the file now (captures fingerprint at this on-disk state).
    let mut b = Session::load_from_path(&path).expect("B loads current state");

    // A writes more, then drops (releases the lease).
    push_text(&mut a, "a-2").expect("A appends newer message");
    push_text(&mut a, "a-3").expect("A appends another newer message");
    drop(a);

    // B is now stale: disk advanced past B's captured fingerprint. Even though
    // the lease is free, B's write must be refused rather than overwrite A's
    // newer a-2/a-3 with B's stale snapshot.
    let b_result = push_text(&mut b, "b-late");
    assert!(
        is_conflict(&b_result),
        "stale writer after drop must be refused by fingerprint mismatch, got {b_result:?}"
    );

    // A's newer messages survive on disk.
    let reloaded = Session::load_from_path(&path).expect("reload after conflict");
    let rendered = reloaded.render_jsonl_snapshot().expect("render");
    assert!(rendered.contains("a-3"), "A's newest message must survive");
    assert!(!rendered.contains("b-late"), "stale write must not have landed");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// A clone of writer A shares the same lease/fingerprint state, so it writes
/// successfully (it is the same logical writer, not a competing one).
#[test]
fn clone_of_writer_shares_lease_and_succeeds() {
    let path = temp_session_path("lease-clone");
    let _ = fs::remove_file(&path);

    let mut a = persisted_session(&path);
    push_text(&mut a, "a-1").expect("A writes and takes lease");

    let mut a_clone = a.clone();
    push_text(&mut a_clone, "clone-2")
        .expect("clone shares lease + fingerprint, so its write succeeds");
    // Original still writes too (shared, refreshed fingerprint).
    push_text(&mut a, "a-3").expect("original continues after clone wrote");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// A fresh reload AFTER the previous writer has fully dropped, with no
/// intervening change, writes successfully: the lease is free and the
/// fingerprint matches current disk.
#[test]
fn fresh_reload_after_drop_writes_successfully() {
    let path = temp_session_path("lease-fresh");
    let _ = fs::remove_file(&path);

    let mut a = persisted_session(&path);
    push_text(&mut a, "a-1").expect("A writes");
    push_text(&mut a, "a-2").expect("A writes");
    drop(a);

    // No other writer touched the file; a fresh load sees current state and can
    // continue.
    let mut b = Session::load_from_path(&path).expect("fresh reload");
    push_text(&mut b, "b-continues")
        .expect("fresh reload with matching fingerprint and free lease writes");
    let reloaded = Session::load_from_path(&path).expect("reload");
    let rendered = reloaded.render_jsonl_snapshot().expect("render");
    assert!(rendered.contains("b-continues"));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// Append (`push_message`) and a full-snapshot rewind on the SAME logical writer
/// cannot lose data: both go through the shared lease/fingerprint, so the
/// rewind's full snapshot is written from current in-memory state and the
/// on-disk file stays consistent (no append clobbered by a stale snapshot).
#[test]
fn append_then_full_snapshot_stays_consistent() {
    let path = temp_session_path("lease-append-snapshot");
    let _ = fs::remove_file(&path);

    let mut a = persisted_session(&path);
    for i in 0..6 {
        push_text(&mut a, &format!("msg-{i}")).expect("append");
    }
    // A full-snapshot rewrite path (save_to_path on the bound path) must succeed
    // and preserve the appended messages it knows about.
    a.save_to_path(&path).expect("bound full snapshot succeeds under lease");
    let reloaded = Session::load_from_path(&path).expect("reload");
    let rendered = reloaded.render_jsonl_snapshot().expect("render");
    for i in 0..6 {
        assert!(rendered.contains(&format!("msg-{i}")), "msg-{i} must survive");
    }
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// Exporting a full snapshot to a DIFFERENT path than the bound one keeps plain
/// overwrite semantics (no lease/fingerprint guard on an explicit destination).
#[test]
fn export_to_other_path_is_unguarded() {
    let bound = temp_session_path("lease-export-bound");
    let export = temp_session_path("lease-export-target");
    let _ = fs::remove_file(&bound);
    let _ = fs::remove_file(&export);

    let mut a = persisted_session(&bound);
    push_text(&mut a, "a-1").expect("bound write");

    // Pre-existing unrelated content at the export path is overwritten freely.
    fs::write(&export, "stale-export-content\n").expect("seed export");
    a.save_to_path(&export).expect("explicit export overwrites without conflict");
    let exported = fs::read_to_string(&export).expect("read export");
    assert!(exported.contains("a-1"));
    assert!(!exported.contains("stale-export-content"));
    let _ = fs::remove_file(&bound);
    let _ = fs::remove_file(&export);
    let _ = fs::remove_file(super::lock_sibling_path(&bound));
}

// --- M1: alias-aware bound-path identity -----------------------------------

/// Saving to a `./`-prefixed or relative alias of the bound path must still be
/// guarded (treated as the bound file), not mistaken for an unrelated export.
#[test]
fn save_through_relative_alias_is_guarded() {
    let path = temp_session_path("alias-rel");
    let _ = fs::remove_file(&path);
    let mut a = persisted_session(&path);
    push_text(&mut a, "a-1").expect("A writes and takes lease");

    // Build a `./`-dotted alias of the same absolute file.
    let parent = path.parent().unwrap();
    let file_name = path.file_name().unwrap();
    let dotted_alias = parent.join(".").join(file_name);
    assert!(a.is_bound_path(&dotted_alias), "dotted alias must be bound");

    // A stale independent writer that targets the file via the alias must be
    // refused, i.e. it cannot bypass the lease/fingerprint by aliasing.
    let b = Session::load_from_path(&path).expect("B loads");
    push_text(&mut a, "a-2").expect("A advances disk");
    drop(a);
    let b_result = b.save_to_path(&dotted_alias);
    assert!(
        is_conflict(&b_result),
        "aliased save by a stale peer must be refused, got {b_result:?}"
    );
    let reloaded = Session::load_from_path(&path).expect("reload");
    assert!(reloaded.render_jsonl_snapshot().unwrap().contains("a-2"));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// A genuinely different, not-yet-created export path stays unguarded (plain
/// overwrite semantics), even though the bound file exists.
#[test]
fn different_nonexistent_export_stays_unguarded() {
    let bound = temp_session_path("alias-bound2");
    let export = temp_session_path("alias-export2");
    let _ = fs::remove_file(&bound);
    let _ = fs::remove_file(&export);
    let mut a = persisted_session(&bound);
    push_text(&mut a, "a-1").expect("bound write");
    assert!(!a.is_bound_path(&export), "distinct export must not be bound");
    a.save_to_path(&export).expect("distinct export is unguarded and succeeds");
    assert!(fs::read_to_string(&export).unwrap().contains("a-1"));
    let _ = fs::remove_file(&bound);
    let _ = fs::remove_file(&export);
    let _ = fs::remove_file(super::lock_sibling_path(&bound));
}

// --- M2: compaction/rewind conflict rollback (no divergence) ---------------

/// When compaction cannot persist because a peer advanced the file, the
/// in-memory session must roll back (messages/anchor unchanged) rather than
/// keep a compacted view that diverges from the newer disk state.
#[test]
fn compaction_conflict_rolls_back_in_memory() {
    let path = temp_session_path("m2-compact");
    let _ = fs::remove_file(&path);

    let mut a = persisted_session(&path);
    for i in 0..4 {
        push_text(&mut a, &format!("a-{i}")).expect("A writes");
    }

    // B loads current state (captures fingerprint), then A advances disk and
    // drops so the lease is free but B is stale.
    let mut b = Session::load_from_path(&path).expect("B loads");
    push_text(&mut a, "a-late").expect("A advances");
    drop(a);

    let before_len = b.messages.len();
    let before_compaction = b.compaction.is_some();
    // B attempts a compaction through the atomic seam: it captures the
    // pre-compaction snapshot before swapping in the compacted set, so a
    // persistence Conflict rolls the original transcript back.
    b.apply_compaction_atomic(
        std::sync::Arc::new(vec![ConversationMessage::user_text("compacted-summary")]),
        "compacted-summary",
        3,
        None,
    );

    // Rolled back: messages and compaction restored to pre-mutation state.
    assert_eq!(b.messages.len(), before_len, "messages must roll back");
    assert_eq!(
        b.compaction.is_some(),
        before_compaction,
        "compaction anchor must roll back"
    );
    // Disk still holds A's newer state, untouched by B.
    let reloaded = Session::load_from_path(&path).expect("reload");
    let rendered = reloaded.render_jsonl_snapshot().unwrap();
    assert!(rendered.contains("a-late"), "A's newer state preserved");
    assert!(!rendered.contains("compacted-summary"), "B's stale compaction not written");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// When rewind cannot persist due to a peer conflict, it must report 0 removed
/// and leave the in-memory messages intact (no divergence).
#[test]
fn rewind_conflict_rolls_back_and_reports_zero() {
    let path = temp_session_path("m2-rewind");
    let _ = fs::remove_file(&path);

    let mut a = persisted_session(&path);
    for i in 0..5 {
        push_text(&mut a, &format!("a-{i}")).expect("A writes");
    }
    let mut b = Session::load_from_path(&path).expect("B loads");
    push_text(&mut a, "a-late").expect("A advances");
    drop(a);

    let before = b.messages.len();
    let removed = b.rewind_turns(1);
    assert_eq!(removed, 0, "conflicting rewind must report nothing removed");
    assert_eq!(b.messages.len(), before, "messages must be intact after rollback");
    let reloaded = Session::load_from_path(&path).expect("reload");
    assert!(reloaded.render_jsonl_snapshot().unwrap().contains("a-late"));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// 거절은 누가 쥐었는지를 이름한다 — 잠금 파일의 도장(pid·cwd·시각)을 읽어서.
/// 도장은 lease 를 잡은 프로세스가 찍고, 놓으면 도장은 남아도 flock 은 없으니
/// `writer_lease_holder` 는 None 이다: 도장은 「누구」이지 「쥐었는가」가 아니다.
#[test]
fn a_refusal_names_the_holder_and_a_free_lease_has_none() {
    let path = temp_session_path("lease-holder");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
    assert_eq!(super::writer_lease_holder(&path), None, "no lock file, no holder");

    let mut first = persisted_session(&path);
    push_text(&mut first, "first").expect("the first writer takes the lease");
    let holder = super::writer_lease_holder(&path).expect("held by this process");
    assert_eq!(holder.pid, std::process::id());
    assert!(holder.at_ms > 0 && holder.cwd.is_some(), "{holder:?}");
    assert!(holder.said().starts_with(&format!("zo pid {}", std::process::id())), "{}", holder.said());

    let mut blocked = persisted_session(&path);
    let refused = push_text(&mut blocked, "blocked");
    assert!(is_conflict(&refused));
    let text = format!("{}", refused.expect_err("refused"));
    assert!(
        text.contains(&format!("writer lease held by zo pid {}", std::process::id())),
        "the refusal names the holder: {text}"
    );
    assert!(text.contains("different session"), "{text}");

    first.release_writer_lease();
    drop(first);
    assert_eq!(
        super::writer_lease_holder(&path),
        None,
        "a stale stamp under a free flock is not a holder"
    );
    assert_eq!(super::held_for(12_000), "12 s");
    assert_eq!(super::held_for(27 * 60_000), "27 min");
    assert_eq!(super::held_for(185 * 60_000), "3 h 05 min");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// **리스 누수 회귀 핀** — 리스는 공유 writer 상태의 수명만큼 유지되고 `Session`
/// 클론이 그 상태를 공유한다. 그래서 런타임이 세션을 갈아끼울 때 옛 세션의 클론이
/// 하나라도 살아 있으면 **버려진 경로가 프로세스가 죽을 때까지 잠긴 채** 남았고,
/// 거기로 `/resume` 하면 있지도 않은 "another process" 를 이유로 매 write 가 실패했다.
/// 핸드오프는 명시적으로 선언한다.
#[test]
fn releasing_the_lease_lets_the_same_process_rebind() {
    let path = temp_session_path("lease-handoff");
    let _ = fs::remove_file(&path);

    let mut first = persisted_session(&path);
    push_text(&mut first, "first").expect("the first writer takes the lease");
    // A surviving clone is what makes the drop below insufficient on its own.
    let clone = first.clone();

    let mut blocked = persisted_session(&path);
    assert!(
        is_conflict(&push_text(&mut blocked, "blocked")),
        "precondition: a competing binder is refused while the lease is held"
    );

    // The handoff: the outgoing session gives the lease back.
    first.release_writer_lease();
    drop(first);

    let mut rebound = Session::load_from_path(&path).expect("reload after the handoff");
    push_text(&mut rebound, "rebound").expect("the same process may bind the path again");

    let reloaded = Session::load_from_path(&path).expect("reload");
    let rendered = reloaded.render_jsonl_snapshot().expect("render");
    assert!(rendered.contains("first") && rendered.contains("rebound"), "{rendered}");
    assert!(!rendered.contains("blocked"), "the refused write never landed: {rendered}");

    // Releasing the lease is not a licence to clobber: the abandoned clone is
    // stale now and the fingerprint guard still refuses it.
    let mut stale = clone;
    assert!(
        is_conflict(&push_text(&mut stale, "stale")),
        "an abandoned writer must still prove the file has not moved on"
    );

    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(super::lock_sibling_path(&path));
}

/// **중복 회귀 핀** — 스냅샷은 세션 *전체*다. 그래서 "파일이 크다"만으로 회전하면
/// 매 full save 가 트랜스크립트를 옆으로 옮기고 **같은 바이트를 그대로 다시 쓴다**:
/// 바이트 단위로 동일한 사본이 3벌까지 쌓여(8.3MB 세션이 33MB) full save 마다
/// 전체 복사를 한 번 더 물었다. 백업은 **버릴 내용이 있을 때만**.
#[test]
fn a_same_size_snapshot_does_not_duplicate_the_transcript() {
    let path = temp_session_path("rotation-no-duplicate");
    for stale in rotation_files(&path) {
        let _ = fs::remove_file(stale);
    }
    let oversized =
        usize::try_from(super::ROTATE_AFTER_BYTES + 10).expect("rotate threshold should fit");
    fs::write(&path, "x".repeat(oversized)).expect("oversized file should write");
    let on_disk = fs::metadata(&path).expect("metadata").len();

    // A snapshot the same size as what is already there discards nothing.
    assert!(
        !rotate_session_file_if_needed(&path, on_disk).expect("rotation check"),
        "an equal-size snapshot must not be backed up"
    );
    // Nor does one that grows (the ordinary case: the session got longer).
    assert!(
        !rotate_session_file_if_needed(&path, on_disk + 4096).expect("rotation check"),
        "a growing snapshot must not be backed up"
    );
    // Nor does a trivial shrink — a header field losing a few bytes is not
    // history being dropped.
    assert!(
        !rotate_session_file_if_needed(&path, on_disk - 16).expect("rotation check"),
        "a trivial shrink must not be backed up"
    );
    assert!(path.exists(), "the live file stayed put");
    assert!(
        rotation_files(&path).is_empty(),
        "no duplicate was written: {:?}",
        rotation_files(&path)
    );

    // Shedding real history still keeps the copy.
    assert!(
        rotate_session_file_if_needed(&path, on_disk - super::ROTATE_MIN_SHED_BYTES)
            .expect("rotation check"),
        "a snapshot that drops a meaningful span is backed up"
    );
    assert_eq!(rotation_files(&path).len(), 1);
    for rotated in rotation_files(&path) {
        let _ = fs::remove_file(rotated);
    }
    let _ = fs::remove_file(&path);
}

/// 헤더만 바뀐 full save 는 8.3MB 세션을 **한 번만** 쓴다 — 예전에는 rename 으로
/// 한 벌 더 남겨서 저장할 때마다 디스크가 두 배로 늘었다.
#[test]
fn a_header_only_full_save_leaves_no_rotated_copy() {
    let path = temp_session_path("rotation-header-save");
    for stale in rotation_files(&path) {
        let _ = fs::remove_file(stale);
    }
    let _ = fs::remove_file(&path);

    let mut session = Session::new().with_persistence_path(path.clone());
    let oversized =
        usize::try_from(super::ROTATE_AFTER_BYTES + 10).expect("rotate threshold should fit");
    session
        .push_user_text("x".repeat(oversized))
        .expect("oversized message appends");

    // Three header changes, each forcing a full snapshot of the same session.
    for name in ["one", "two", "three"] {
        session.name = Some(name.to_string());
        session.save_to_path(&path).expect("header change persists");
    }

    assert!(
        rotation_files(&path).is_empty(),
        "a full save of an unchanged transcript must not duplicate it: {:?}",
        rotation_files(&path)
    );
    let reloaded = Session::load_from_path(&path).expect("reload");
    assert_eq!(reloaded.name.as_deref(), Some("three"));
    let _ = fs::remove_file(&path);
}

#[test]
fn rewind_pops_persisted_reminder_annotations_with_their_turn() {
    // The runtime persists each request's reminder set as a System message
    // (`<system-reminder>`-tagged text). Rewinding a turn must treat those as
    // part of the turn — while a compaction summary (System WITHOUT the tag)
    // still stops the walk and survives.
    let reminder = || ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text {
            text: "<system-reminder>\ntodo: item A\n</system-reminder>".to_string(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    };
    let summary = ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text {
            text: "Compacted continuation summary of earlier work".to_string(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    };

    let mut session = Session::new();
    session.push_message(summary.clone()).unwrap();
    session.push_user_text("do the thing").unwrap();
    session.push_message(reminder()).unwrap();
    session
        .push_message(ConversationMessage::assistant(vec![
            ContentBlock::Text {
                text: "running".into(),
            },
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: "true".into(),
            },
        ]))
        .unwrap();
    session.push_message(make_tool_result_message("t1")).unwrap();
    session.push_message(reminder()).unwrap();
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "done".into(),
        }]))
        .unwrap();
    assert_eq!(session.messages.len(), 7);

    // steps counts assistant iterations (see the fn doc): two assistant
    // messages → two steps unwind the whole user turn, reminders included.
    let removed = session.rewind_turns(2);
    assert_eq!(
        removed, 6,
        "turn + its reminder annotations + tool results must all rewind"
    );
    assert_eq!(session.messages.len(), 1);
    assert_eq!(
        session.messages[0], summary,
        "the compaction summary is not a turn annotation and must survive"
    );

    // Rewinding past the summary removes nothing more: the summary is not a
    // turn, and the walk must not consume it as one.
    let removed_again = session.rewind_turns(1);
    assert_eq!(removed_again, 0);
    assert_eq!(session.messages.len(), 1);
}

// --- append fingerprint stays O(appended bytes) -----------------------------

fn expected_fingerprint(session: &Session) -> Option<super::FileFingerprint> {
    let persistence = session.persistence.as_ref()?;
    let state = persistence.writer.lock().ok()?;
    state.expected.clone()
}

fn full_file_reads() -> u64 {
    super::FULL_FILE_READS.with(std::cell::Cell::get)
}

fn cleanup_session_file(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(super::lock_sibling_path(path));
}

/// Every `push_message` used to read and digest the ENTIRE transcript twice —
/// once to recheck the writer guard and once to refresh it — so a session's
/// total persistence cost grew quadratically with its own length. Steady-state
/// appends must now touch only the bytes they add.
///
/// Unix only: the identity stamp that makes the skip safe needs an inode, and
/// without an advisory lock to lean on the other platforms deliberately keep
/// paying for the full compare.
#[cfg(unix)]
#[test]
fn steady_state_appends_never_re_read_the_whole_transcript() {
    let path = temp_session_path("append-no-full-reads");
    let mut session = Session::new().with_persistence_path(path.clone());
    // Bootstrap and the first append are allowed to look at the file; what must
    // stay O(appended bytes) is the steady state that follows.
    push_text(&mut session, "seed").expect("seed append");
    push_text(&mut session, "arm the writer guard").expect("second append");

    let before = full_file_reads();
    for turn in 0..16 {
        push_text(&mut session, &format!("steady turn {turn}")).expect("steady append");
    }
    let reads = full_file_reads() - before;

    assert_eq!(
        reads, 0,
        "16 steady-state appends must not re-read the transcript, but performed {reads} whole-file reads"
    );
    cleanup_session_file(&path);
}

/// The incremental fingerprint is an optimization only if it is exact: it must
/// equal, bit for bit, what re-reading and digesting the file would produce.
#[test]
fn incremental_append_fingerprint_equals_a_full_re_read() {
    let path = temp_session_path("append-fingerprint-equivalence");
    let mut session = Session::new().with_persistence_path(path.clone());
    for turn in 0..20 {
        // Vary the payload length so a byte-count slip cannot cancel out.
        let payload = "payload ".repeat(turn % 7 + 1);
        push_text(&mut session, payload.trim_end()).expect("append should persist");
        let tracked = expected_fingerprint(&session).expect("writer tracks a fingerprint");
        let on_disk = super::FileFingerprint::of_path(&path)
            .expect("session file should be readable")
            .expect("session file should exist");
        assert_eq!(
            tracked, on_disk,
            "incremental fingerprint diverged from the file after append {turn}"
        );
    }
    cleanup_session_file(&path);
}

/// The fast path must never swallow a peer rewrite. An in-place overwrite
/// changes the length; a temp-file-plus-rename replacement (what every rewrite
/// path in this module actually does) changes the inode. Both must still be
/// refused with `Conflict`.
#[test]
fn peer_rewrite_between_appends_still_conflicts() {
    let in_place = temp_session_path("append-guard-in-place");
    let mut session = Session::new().with_persistence_path(in_place.clone());
    push_text(&mut session, "durable before peer rewrite").expect("seed append");
    push_text(&mut session, "second append arms the fast path").expect("second append");
    fs::write(&in_place, "peer replaced this session\n").expect("simulate in-place peer rewrite");
    assert!(
        is_conflict(&push_text(&mut session, "after peer rewrite")),
        "an in-place peer rewrite must still be refused"
    );
    cleanup_session_file(&in_place);

    let renamed = temp_session_path("append-guard-rename");
    let mut session = Session::new().with_persistence_path(renamed.clone());
    push_text(&mut session, "durable before peer swap").expect("seed append");
    push_text(&mut session, "second append arms the fast path").expect("second append");
    let original = fs::read(&renamed).expect("read the transcript we just wrote");
    // Same byte length, different content, published by rename — so only the
    // inode betrays the swap.
    let swapped = {
        let mut bytes = original.clone();
        let last = bytes.len() - 1;
        bytes[..last].iter_mut().for_each(|byte| *byte = b'x');
        bytes
    };
    assert_eq!(swapped.len(), original.len());
    assert_ne!(swapped, original);
    let staging = renamed.with_extension("peer-staging");
    fs::write(&staging, &swapped).expect("stage the peer replacement");
    fs::rename(&staging, &renamed).expect("publish the peer replacement by rename");
    assert!(
        is_conflict(&push_text(&mut session, "after peer swap")),
        "a same-length rename-published peer rewrite must still be refused"
    );
    cleanup_session_file(&renamed);
}

/// A full snapshot replaces the file underneath the writer. The identity stamp
/// must be re-armed from the snapshot, not left pointing at the pre-snapshot
/// file, or the next append would trust a fingerprint for a file that is gone.
#[test]
fn append_after_a_full_snapshot_keeps_the_fingerprint_exact() {
    let path = temp_session_path("append-after-snapshot");
    let mut session = Session::new().with_persistence_path(path.clone());
    push_text(&mut session, "before snapshot").expect("seed append");

    session.name = Some("forces a full snapshot".to_string());
    session.save_to_path(&path).expect("full snapshot should publish");

    push_text(&mut session, "after snapshot").expect("append after snapshot should persist");
    let tracked = expected_fingerprint(&session).expect("writer tracks a fingerprint");
    let on_disk = super::FileFingerprint::of_path(&path)
        .expect("session file should be readable")
        .expect("session file should exist");
    assert_eq!(
        tracked, on_disk,
        "the fingerprint must stay exact across a snapshot boundary"
    );

    let reloaded = Session::load_from_path(&path).expect("snapshot + append must reload");
    assert_eq!(reloaded.messages.len(), 2);
    cleanup_session_file(&path);
}

/// Adversarial review's sharpest case: an in-place rewrite that keeps the byte
/// length AND restores the original mtime. Length, inode and mtime all read
/// back unchanged, so a stamp built from those three alone would wave it
/// through and append onto a transcript that is no longer ours. `ctime` is the
/// backstop — `utimensat` can forge mtime but bumps ctime doing it — so the
/// guard must still refuse.
#[cfg(unix)]
#[test]
fn same_length_in_place_rewrite_with_restored_mtime_still_conflicts() {
    let path = temp_session_path("append-guard-forged-mtime");
    let mut session = Session::new().with_persistence_path(path.clone());
    push_text(&mut session, "durable before forged rewrite").expect("seed append");
    push_text(&mut session, "second append arms the fast path").expect("second append");

    let before = fs::metadata(&path).expect("stat the transcript we wrote");
    let original_modified = before.modified().expect("mtime should be readable");
    let original_accessed = before.accessed().expect("atime should be readable");
    let original_inode = std::os::unix::fs::MetadataExt::ino(&before);

    let original = fs::read(&path).expect("read the transcript we wrote");
    let mut forged = original.clone();
    let last = forged.len() - 1;
    forged[..last].iter_mut().for_each(|byte| *byte = b'z');
    assert_eq!(forged.len(), original.len(), "the forgery must keep the length");
    assert_ne!(forged, original);
    // Truncate-and-write in place, so the inode is preserved.
    fs::write(&path, &forged).expect("rewrite in place");
    std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("reopen to restore times")
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(original_modified)
                .set_accessed(original_accessed),
        )
        .expect("restore the original mtime");

    let after = fs::metadata(&path).expect("stat the forged transcript");
    assert_eq!(after.len(), before.len(), "length must be indistinguishable");
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&after),
        original_inode,
        "inode must be indistinguishable"
    );
    assert_eq!(
        after.modified().expect("mtime"),
        original_modified,
        "mtime must be indistinguishable"
    );

    assert!(
        is_conflict(&push_text(&mut session, "after forged rewrite")),
        "a same-length, mtime-restored in-place rewrite must still be refused"
    );
    cleanup_session_file(&path);
}

/// Handing the writer lease back means someone else may legitimately publish,
/// so the fast path must stand down: the next append has to pay for the full
/// freshness compare again.
#[cfg(unix)]
#[test]
fn releasing_the_writer_lease_forces_the_next_append_to_re_read() {
    let path = temp_session_path("append-lease-release-disarms");
    let mut session = Session::new().with_persistence_path(path.clone());
    push_text(&mut session, "seed").expect("seed append");
    push_text(&mut session, "arm the fast path").expect("second append");

    let armed_before = full_file_reads();
    push_text(&mut session, "still armed").expect("third append");
    assert_eq!(
        full_file_reads() - armed_before,
        0,
        "an armed writer must not re-read the transcript"
    );

    session.release_writer_lease();
    let released_before = full_file_reads();
    push_text(&mut session, "after lease release").expect("append after lease release");
    assert!(
        full_file_reads() - released_before > 0,
        "after handing the lease back the next append must re-verify the file"
    );
    cleanup_session_file(&path);
}

/// The secure persistence mode takes a different write branch (descriptor
/// validated open plus `sync_all`), so its incremental fingerprint needs its
/// own proof that it tracks the bytes actually written. Unix only for the same
/// reason as the plain steady-state test.
#[cfg(unix)]
#[test]
fn secure_persistence_appends_keep_the_fingerprint_exact_without_re_reading() {
    let path = temp_session_path("append-secure-incremental");
    let mut session = Session::new().with_secure_persistence_path(path.clone());
    push_text(&mut session, "secure seed").expect("seed append");
    push_text(&mut session, "secure arm").expect("second append");

    let before = full_file_reads();
    for turn in 0..8 {
        let payload = "secure payload ".repeat(turn % 5 + 1);
        push_text(&mut session, payload.trim_end()).expect("secure append should persist");
    }
    assert_eq!(
        full_file_reads() - before,
        0,
        "secure appends must not re-read the transcript either"
    );

    let tracked = expected_fingerprint(&session).expect("writer tracks a fingerprint");
    let on_disk = super::FileFingerprint::of_path(&path)
        .expect("session file should be readable")
        .expect("session file should exist");
    assert_eq!(
        tracked, on_disk,
        "the secure branch's incremental fingerprint must match the file"
    );
    cleanup_session_file(&path);
}

/// A real ordinary turn is `push_message` for each new message AND one
/// `persist_appended_state_to_path` to close the turn. Measuring only the
/// appends would have hidden a whole-transcript read still sitting on the
/// turn-final call, which grows with the session exactly like the one the
/// append path just shed — only counted in turns.
#[cfg(unix)]
#[test]
fn whole_ordinary_turns_never_re_read_the_whole_transcript() {
    let path = temp_session_path("ordinary-turn-no-full-reads");
    let mut session = Session::new().with_persistence_path(path.clone());
    session.push_user_text("seed the store").expect("seed append");
    session
        .persist_appended_state_to_path(&path)
        .expect("seed turn should persist");

    let before = full_file_reads();
    for turn in 0..8 {
        session
            .push_user_text(format!("user turn {turn}"))
            .expect("user append");
        session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: format!("assistant turn {turn}"),
            }]))
            .expect("assistant append");
        session
            .persist_appended_state_to_path(&path)
            .expect("ordinary turn should persist");
    }
    let reads = full_file_reads() - before;

    assert_eq!(
        reads, 0,
        "8 whole ordinary turns must not re-read the transcript, but performed {reads} whole-file reads"
    );
    cleanup_session_file(&path);
}

/// Pins the snapshot re-arm itself, not just the fingerprint it leaves behind:
/// drop `arm_identity_after_publish` from the guarded write and the append
/// after a snapshot silently falls back to the whole-file compare.
#[cfg(unix)]
#[test]
fn a_full_snapshot_re_arms_the_fast_path_for_the_next_append() {
    let path = temp_session_path("snapshot-re-arms-fast-path");
    let mut session = Session::new().with_persistence_path(path.clone());
    push_text(&mut session, "before snapshot").expect("seed append");
    session.name = Some("forces a full snapshot".to_string());
    session.save_to_path(&path).expect("full snapshot should publish");

    let before = full_file_reads();
    push_text(&mut session, "after snapshot").expect("append after snapshot should persist");
    assert_eq!(
        full_file_reads() - before,
        0,
        "a snapshot must re-arm the stamp so the next append still skips the re-read"
    );
    cleanup_session_file(&path);
}
