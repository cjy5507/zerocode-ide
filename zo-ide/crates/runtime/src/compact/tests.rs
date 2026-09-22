use super::{
    apply_compaction, bound_anchor, collect_key_files, compact_session, compact_session_with,
    distill_session_state, edited_file_paths, estimate_session_tokens, fold_anchor,
    format_compact_summary, get_compact_continuation_message, infer_pending_work,
    is_edit_result_tool, microcompact_clearable_estimate, microcompact_quote,
    microcompact_session,
    parse_anchor_from_summary, prepare_compaction, render_anchor_to_summary_text, should_compact,
    summary_fabricates_identifiers, ANCHOR_SECTION_MAX_CHARS, CompactionConfig,
    CompactionSummarizer, LocalSummarizer, MICROCOMPACT_PLACEHOLDER,
    MICROCOMPACT_REMINDER_PLACEHOLDER, PENDING_TASKS_MAX_CHARS,
};
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

/// One user message holding a single tool result with the given body/images.
fn tool_result_message(id: usize, output: &str, image_count: usize) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: format!("tool-{id}"),
            tool_name: "Read".to_string(),
            output: output.to_string(),
            is_error: false,
            images: (0..image_count)
                .map(|i| (format!("image/png-{i}"), "data".to_string()))
                .collect(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    }
}

/// One user message holding a single `edit_file` tool result whose JSON
/// envelope mutates `path`, with a large body so it would otherwise be
/// microcompact-clearable.
fn edit_result_message(id: usize, path: &str) -> ConversationMessage {
    let body = serde_json::json!({
        "filePath": path,
        "structuredPatch": [{
            "oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1,
            "lines": [format!("-old {}", "x".repeat(600)), format!("+new {}", "y".repeat(600))],
        }],
    })
    .to_string();
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: format!("edit-{id}"),
            tool_name: "edit_file".to_string(),
            output: body,
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
fn state_distill_summarizes_working_state_without_mutating_transcript() {
    let mut session = Session::new();
    session
        .push_user_text(
            "Implement StateDistill next in crates/runtime/src/conversation/compaction.rs; TODO verify cargo test.",
        )
        .expect("user message");
    session
        .push_message(ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::Text {
                text: "Current work: wiring the state_distill threshold before full compaction.".to_string(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        })
        .expect("assistant message");
    let before = session.messages.clone();

    let distilled = distill_session_state(&session).expect("state distill should have content");

    assert_eq!(
        session.messages, before,
        "state distill must not remove or rewrite transcript messages"
    );
    assert!(distilled.contains("# Distilled working state"));
    assert!(distilled.contains("Current work"));
    assert!(distilled.contains("Recent user requests"));
    assert!(distilled.contains("Pending/next signals"));
    assert!(
        distilled.contains("crates/runtime/src/conversation/compaction.rs"),
        "distilled state should preserve grounded file identifiers: {distilled}"
    );
}

#[test]
fn is_edit_result_tool_matches_mutation_tools_only() {
    for name in [
        "edit_file",
        "write_file",
        "Edit",
        "MultiEdit",
        "Write",
        "NotebookEdit",
        // MCP file servers / editor plugins are namespaced (`mcp__<server>__<leaf>`
        // or `<plugin>__<leaf>`) — the leaf verb must still count as a mutation so
        // the applied edit survives the trim instead of being cleared and reverted.
        "mcp__fs__write_file",
        "mcp__filesystem__edit_file",
        "myeditor__write_file",
    ] {
        assert!(is_edit_result_tool(name), "{name} must count as a mutation");
    }
    for name in [
        "read_file",
        "Read",
        "bash",
        "grep_search",
        "Grep",
        "TodoWrite",
        // Namespaced NON-file tools must NOT be misclassified: leaf verbs outside
        // the file-edit set (`search`, `create_entities`, `move_file`) stay reads.
        "mcp__memory__search",
        "mcp__db__create_entities",
        "mcp__fs__move_file",
    ] {
        assert!(
            !is_edit_result_tool(name),
            "{name} must NOT count as a mutation"
        );
    }
}

#[test]
fn microcompact_preserves_mcp_edit_result_bodies() {
    // Same fix as builtin edits, via an MCP file server: 8 large
    // `mcp__fs__write_file` results with keep_recent=5. If the namespaced leaf
    // were not recognized as a mutation, the 3 oldest would be cleared and the
    // model could revert its own MCP-applied edit after compaction.
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(
        (0..8)
            .map(|i| {
                let mut message = edit_result_message(i, &format!("src/mcp_{i}.rs"));
                if let Some(ContentBlock::ToolResult { tool_name, .. }) =
                    message.blocks.first_mut()
                {
                    *tool_name = "mcp__fs__write_file".to_string();
                }
                message
            })
            .collect(),
    );
    assert!(
        microcompact_session(&mut session, 5, 240).is_none(),
        "MCP file-edit diffs must be exempt from the microcompact trim"
    );
    assert!(session.messages.iter().all(|message| {
        matches!(
            message.blocks.first(),
            Some(ContentBlock::ToolResult { output, .. }) if output != MICROCOMPACT_PLACEHOLDER
        )
    }));
}

#[test]
fn microcompact_never_clears_edit_write_result_bodies() {
    // 8 large edit_file results: with keep_recent=5 the 3 oldest would be
    // cleared if they were ordinary results — but edit/write diffs are exempt,
    // so the trim finds nothing clearable and returns None. This is the fix for
    // "the model loses (and reverts) its own applied edit after compaction".
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(
        (0..8)
            .map(|i| edit_result_message(i, &format!("src/file_{i}.rs")))
            .collect(),
    );
    assert!(
        microcompact_session(&mut session, 5, 240).is_none(),
        "edit/write diffs must be exempt from the microcompact trim"
    );
    // Every edit body is intact (none replaced by the placeholder).
    assert!(session.messages.iter().all(|message| {
        matches!(
            message.blocks.first(),
            Some(ContentBlock::ToolResult { output, .. }) if output != MICROCOMPACT_PLACEHOLDER
        )
    }));
}

#[test]
fn microcompact_still_clears_reads_but_preserves_interleaved_edits() {
    // Mix: old reads (clearable) interleaved with an old edit (exempt). Only the
    // reads beyond the recent tail get cleared; the edit diff survives.
    let big_read = "r".repeat(1_000);
    let mut session = Session::new();
    let mut messages = vec![
        tool_result_message(0, &big_read, 0),  // old read -> clearable
        edit_result_message(1, "src/keep.rs"), // old edit -> exempt
        tool_result_message(2, &big_read, 0),  // old read -> clearable
    ];
    // Recent tail of exactly keep_recent (5) clearable reads that must survive.
    messages.extend((3..8).map(|i| tool_result_message(i, &big_read, 0)));
    session.messages = ::std::sync::Arc::new(messages);

    let event = microcompact_session(&mut session, 5, 240).expect("2 old reads clearable");
    assert_eq!(event.cleared_results, 2, "only the two old reads clear");
    // The edit diff at index 1 is untouched and still names its file.
    match session.messages[1].blocks.first() {
        Some(ContentBlock::ToolResult { output, .. }) => {
            assert_ne!(output, MICROCOMPACT_PLACEHOLDER, "edit body preserved");
            assert!(output.contains("src/keep.rs"));
        }
        other => panic!("unexpected block: {other:?}"),
    }
}

#[test]
fn microcompact_clears_old_standalone_images_but_keeps_recent() {
    // 7 user-pasted images (~1,600 tok each re-sent EVERY request): with
    // keep_recent=5 the 2 oldest become text placeholders; the recent 5 stay
    // pixels. No tool results at all — image clearing must fire on its own.
    let image_message = |i: usize| ConversationMessage {
        role: MessageRole::User,
        blocks: vec![
            ContentBlock::Text {
                text: format!("look at screenshot {i}"),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
            },
        ],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    };
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new((0..7).map(image_message).collect());

    let event = microcompact_session(&mut session, 5, 240).expect("2 old images clearable");
    assert_eq!(event.cleared_results, 0);
    assert_eq!(event.cleared_images, 2);
    assert!(event.estimated_tokens_saved >= 3_200);
    for (index, message) in session.messages.iter().enumerate() {
        let replaced = matches!(
            &message.blocks[1],
            ContentBlock::Text { text } if text == super::MICROCOMPACT_IMAGE_PLACEHOLDER
        );
        assert_eq!(replaced, index < 2, "only the 2 oldest images clear (index {index})");
    }

    // Idempotent: the placeholders are Text blocks now, so a second pass finds
    // only the kept 5 images — nothing beyond keep_recent — and does nothing.
    assert!(microcompact_session(&mut session, 5, 240).is_none());
}

#[test]
fn edited_file_paths_extracts_distinct_paths_in_first_seen_order() {
    let messages = vec![
        edit_result_message(0, "src/a.rs"),
        tool_result_message(1, "just a read", 0), // ignored: not a mutation
        edit_result_message(2, "src/b.rs"),
        edit_result_message(3, "src/a.rs"), // duplicate path -> deduped
    ];
    assert_eq!(
        edited_file_paths(&messages),
        vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
    );
}

#[test]
fn edited_file_paths_skips_cleared_and_unparseable_envelopes() {
    // A cleared placeholder body and a non-JSON body both contribute nothing,
    // never panic.
    let cleared = ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: "e1".to_string(),
            tool_name: "edit_file".to_string(),
            output: MICROCOMPACT_PLACEHOLDER.to_string(),
            is_error: false,
            images: Vec::new(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    };
    let garbage = ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: "e2".to_string(),
            tool_name: "write_file".to_string(),
            output: "not json".to_string(),
            is_error: false,
            images: Vec::new(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    };
    assert!(edited_file_paths(&[cleared, garbage]).is_empty());
}

#[test]
fn microcompact_clears_old_results_and_keeps_the_recent_tail() {
    let mut session = Session::new();
    let big = "x".repeat(1_000);
    session.messages =
        ::std::sync::Arc::new((0..8).map(|i| tool_result_message(i, &big, 0)).collect());

    let event = microcompact_session(&mut session, 5, 240).expect("3 old results clearable");
    assert_eq!(event.cleared_results, 3);
    // (1000 - placeholder) / 4 per cleared result, no images.
    let per_result = (1_000 - MICROCOMPACT_PLACEHOLDER.len() as u64) / 4;
    assert_eq!(event.estimated_tokens_saved, 3 * per_result);

    let bodies: Vec<&str> = session
        .messages
        .iter()
        .filter_map(|message| match message.blocks.first() {
            Some(ContentBlock::ToolResult { output, .. }) => Some(output.as_str()),
            _ => None,
        })
        .collect();
    assert!(bodies[..3].iter().all(|b| *b == MICROCOMPACT_PLACEHOLDER));
    assert!(bodies[3..].iter().all(|b| *b == big), "recent 5 untouched");

    // Idempotent: the cleared placeholders are never counted again.
    assert!(microcompact_session(&mut session, 5, 240).is_none());
}

/// [`microcompact_clearable_estimate`] must be read exactly what
/// [`microcompact_session`] is about to do — the two share one candidate
/// selection ([`super::plan_microcompact_clears`]) precisely so a caller
/// gating on the estimate is never quoted a number the actual clear then
/// misses.
///
/// The transcript deliberately straddles the mutation frontier: a superseded
/// reminder ahead of the first cleared tool result (which the frontier rule
/// strands) and one behind it (which rides along for free). A quote that
/// counted the stranded copy would promise the caller tokens the clear never
/// frees, which is exactly how a break-even gate starts approving losing trades.
#[test]
fn microcompact_clearable_estimate_matches_actual_clear() {
    let reminder = format!("<system-reminder>\n{}\n</system-reminder>", "nag ".repeat(400));
    let big = "x".repeat(1_000);
    let mut messages = vec![reminder_message(&reminder)];
    messages.extend((0..8).map(|i| tool_result_message(i, &big, 0)));
    messages.push(reminder_message(&reminder));
    messages.push(reminder_message(&reminder));
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(messages);

    let estimate = microcompact_clearable_estimate(&session, 5, 240);
    assert!(estimate > 0, "8 big results with a keep-5 budget must be clearable");

    let event = microcompact_session(&mut session, 5, 240).expect("3 old results clearable");
    assert_eq!(event.cleared_results, 3);
    assert_eq!(event.cleared_superseded_reminders, 1);
    assert_eq!(
        estimate, event.estimated_tokens_saved,
        "the read-only estimate must equal what the clear actually reports"
    );
}

#[test]
fn microcompact_session_second_call_with_same_keep_recent_is_a_true_no_op() {
    // Strengthens the "returns None" idempotency spot-checks above: the
    // no-new-clearable re-call must leave the session byte-for-byte
    // unchanged, not merely report nothing new. `Session` derives a
    // field-by-field `PartialEq` (excluding the persistence handle), so this
    // is a genuine deep-equality check across every message/block.
    let mut session = Session::new();
    let big = "x".repeat(1_000);
    session.messages =
        ::std::sync::Arc::new((0..8).map(|i| tool_result_message(i, &big, 0)).collect());

    let event = microcompact_session(&mut session, 5, 240).expect("3 old results clearable");
    assert_eq!(event.cleared_results, 3);

    let after_first_pass = session.clone();
    let second = microcompact_session(&mut session, 5, 240);
    assert!(second.is_none(), "nothing new to clear on the second pass");
    assert_eq!(
        session, after_first_pass,
        "re-running with the same keep_recent must not mutate the session at all"
    );
}

#[test]
fn microcompact_skips_small_bodies_but_always_clears_images() {
    let mut session = Session::new();
    let mut messages = vec![
        tool_result_message(0, "tiny", 0), // small, no image → protected
        tool_result_message(1, "tiny-img", 2), // small but 2 images → clearable
        tool_result_message(2, &"y".repeat(500), 0), // big → clearable
    ];
    // Recent tail of exactly keep_recent clearable results that must survive.
    messages.extend((3..8).map(|i| tool_result_message(i, &"z".repeat(500), 0)));
    session.messages = ::std::sync::Arc::new(messages);

    let event = microcompact_session(&mut session, 5, 240).expect("2 clearable beyond tail");
    assert_eq!(event.cleared_results, 2);
    // Image clearing dominates the estimate: 2 × 1,600.
    assert!(event.estimated_tokens_saved >= 3_200);

    match session.messages[1].blocks.first() {
        Some(ContentBlock::ToolResult { output, images, .. }) => {
            assert_eq!(output, MICROCOMPACT_PLACEHOLDER);
            assert!(images.is_empty(), "images cleared with the body");
        }
        other => panic!("unexpected block: {other:?}"),
    }
    match session.messages[0].blocks.first() {
        Some(ContentBlock::ToolResult { output, .. }) => assert_eq!(output, "tiny"),
        other => panic!("unexpected block: {other:?}"),
    }
}

#[test]
fn microcompact_returns_none_when_everything_is_recent() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(
        (0..4)
            .map(|i| tool_result_message(i, &"w".repeat(500), 0))
            .collect(),
    );
    assert!(microcompact_session(&mut session, 5, 240).is_none());
}

#[test]
fn formats_compact_summary_like_upstream() {
    let summary = "<analysis>scratch</analysis>\n<summary>Kept work</summary>";
    assert_eq!(format_compact_summary(summary), "Summary:\nKept work");
}

#[test]
fn leaves_small_sessions_unchanged() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![ConversationMessage::user_text("hello")]);

    let result = compact_session(&session, CompactionConfig::default());
    assert_eq!(result.removed_message_count, 0);
    assert_eq!(result.compacted_session, session);
    assert!(result.summary.is_empty());
    assert!(result.formatted_summary.is_empty());
}

#[test]
fn compacts_older_messages_into_a_system_summary() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("one ".repeat(200)),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "two ".repeat(200),
        }]),
        ConversationMessage::user_text("three ".repeat(200)),
        ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::Text {
                text: "recent".to_string(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        },
    ]);

    let result = compact_session(
        &session,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        },
    );

    assert_eq!(result.removed_message_count, 2);
    assert_eq!(
        result.compacted_session.messages[0].role,
        MessageRole::System
    );
    assert!(matches!(
        &result.compacted_session.messages[0].blocks[0],
        ContentBlock::Text { text } if text.contains("Summary:")
    ));
    assert!(result.formatted_summary.contains("Scope:"));
    assert!(result.formatted_summary.contains("Key timeline:"));
    assert!(should_compact(
        &session,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        }
    ));
    assert!(estimate_session_tokens(&result.compacted_session) < estimate_session_tokens(&session));
}

#[test]
fn keeps_previous_compacted_context_when_compacting_again() {
    let mut initial_session = Session::new();
    initial_session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("Investigate rust/crates/runtime/src/compact.rs"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "I will inspect the compact flow.".to_string(),
        }]),
        ConversationMessage::user_text("Also update rust/crates/runtime/src/conversation.rs"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Next: preserve prior summary context during auto compact.".to_string(),
        }]),
    ]);
    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
    };

    let first = compact_session(&initial_session, config);
    let mut follow_up_messages = first.compacted_session.messages.to_vec();
    follow_up_messages.extend([
        ConversationMessage::user_text("Please add regression tests for compaction."),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Working on regression coverage now.".to_string(),
        }]),
    ]);

    let mut second_session = Session::new();
    second_session.messages = ::std::sync::Arc::new(follow_up_messages);
    let second = compact_session(&second_session, config);

    // LAVA P1 drift-free contract: round-1 content (compacted in round 1)
    // survives verbatim into round 2's summary via the typed anchor's verbatim
    // carry-forward, and round-2 content is folded in alongside it.
    assert!(
        second.formatted_summary.contains("compact.rs"),
        "round-1 content must carry forward through round 2"
    );
    assert!(
        second.formatted_summary.contains("conversation.rs"),
        "round-2 content must be folded into the same summary"
    );
    // The continuation system message carries both rounds' context too.
    assert!(matches!(
        &second.compacted_session.messages[0].blocks[0],
        ContentBlock::Text { text }
            if text.contains("compact.rs") && text.contains("conversation.rs")
    ));
    assert!(matches!(
        &second.compacted_session.messages[1].blocks[0],
        ContentBlock::Text { text } if text.contains("Please add regression tests for compaction.")
    ));
}

#[test]
fn ignores_existing_compacted_summary_when_deciding_to_recompact() {
    let summary = "<summary>Conversation summary:\n- Scope: earlier work preserved.\n- Key timeline:\n  - user: large preserved context\n</summary>";
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage {
            role: MessageRole::System,
            blocks: vec![ContentBlock::Text {
                text: get_compact_continuation_message(summary, true, true, &[]),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        },
        ConversationMessage::user_text("tiny"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "recent".to_string(),
        }]),
    ]);

    assert!(!should_compact(
        &session,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        }
    ));
}

#[test]
fn truncates_long_blocks_in_summary() {
    let summary = super::summarize_block(&ContentBlock::Text {
        text: "x".repeat(400),
    });
    assert!(summary.ends_with('…'));
    assert!(summary.chars().count() <= 161);
}

#[test]
fn extracts_key_files_from_message_content() {
    let files = collect_key_files(&[ConversationMessage::user_text(
        "Update rust/crates/runtime/src/compact.rs and rust/crates/zo-cli/src/main.rs next.",
    )]);
    assert!(files.contains(&"rust/crates/runtime/src/compact.rs".to_string()));
    assert!(files.contains(&"rust/crates/zo-cli/src/main.rs".to_string()));
}

#[test]
fn infers_pending_work_from_recent_messages() {
    let pending = infer_pending_work(&[
        ConversationMessage::user_text("done"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Next: update tests and follow up on remaining CLI polish.".to_string(),
        }]),
    ]);
    assert_eq!(pending.len(), 1);
    assert!(pending[0].contains("Next: update tests"));
}

// --- boundary cases ---

#[test]
fn compact_session_with_zero_messages_returns_unchanged() {
    let session = Session::new();
    let result = compact_session(&session, CompactionConfig::default());
    assert_eq!(result.removed_message_count, 0);
    assert_eq!(result.compacted_session, session);
    assert!(result.summary.is_empty());
}

#[test]
fn compact_session_with_one_message_returns_unchanged() {
    let mut session = Session::new();
    session.messages =
        ::std::sync::Arc::new(vec![ConversationMessage::user_text("single message")]);
    let result = compact_session(&session, CompactionConfig::default());
    assert_eq!(result.removed_message_count, 0);
    assert_eq!(result.compacted_session, session);
}

#[test]
fn compact_session_fewer_messages_than_preserve_recent_returns_unchanged() {
    // preserve_recent_messages = 4 (default), only 3 messages — never compacts
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("a ".repeat(500)),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "b ".repeat(500),
        }]),
        ConversationMessage::user_text("c ".repeat(500)),
    ]);
    // Even with a very low token threshold, fewer messages than preserve window means no compaction.
    let result = compact_session(
        &session,
        CompactionConfig {
            preserve_recent_messages: 4,
            max_estimated_tokens: 1,
        },
    );
    assert_eq!(result.removed_message_count, 0);
}

#[test]
fn should_compact_returns_false_when_below_token_threshold() {
    let mut session = Session::new();
    // 5 short messages — token count will be low
    for i in 0..5 {
        ::std::sync::Arc::make_mut(&mut session.messages)
            .push(ConversationMessage::user_text(format!("message {i}")));
    }
    assert!(!should_compact(
        &session,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 100_000,
        }
    ));
}

#[test]
fn should_compact_returns_true_when_above_token_threshold() {
    let mut session = Session::new();
    // 6 messages each with ~300 tokens worth of text (1200 bytes / 4 = 300)
    for _ in 0..6 {
        ::std::sync::Arc::make_mut(&mut session.messages)
            .push(ConversationMessage::user_text("w ".repeat(600)));
    }
    assert!(should_compact(
        &session,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 100,
        }
    ));
}

// --- large session simulation ---

#[test]
fn compact_session_fifty_messages_preserves_recent_tail() {
    let mut session = Session::new();
    for i in 0..50u32 {
        if i % 2 == 0 {
            ::std::sync::Arc::make_mut(&mut session.messages).push(ConversationMessage::user_text(
                format!("user turn {i} ") + &"x ".repeat(50),
            ));
        } else {
            ::std::sync::Arc::make_mut(&mut session.messages).push(ConversationMessage::assistant(
                vec![ContentBlock::Text {
                    text: format!("assistant turn {i} ") + &"y ".repeat(50),
                }],
            ));
        }
    }

    let config = CompactionConfig {
        preserve_recent_messages: 4,
        max_estimated_tokens: 100,
    };

    assert!(should_compact(&session, config));

    let result = compact_session(&session, config);

    // first message must be the synthetic system summary
    assert_eq!(
        result.compacted_session.messages[0].role,
        MessageRole::System
    );

    // exactly preserve_recent_messages messages follow the summary
    assert_eq!(
        result.compacted_session.messages.len(),
        config.preserve_recent_messages + 1
    );

    // the preserved tail must match the last N original messages verbatim
    let tail_start = 50 - config.preserve_recent_messages;
    for (offset, original_idx) in (tail_start..50).enumerate() {
        assert_eq!(
            result.compacted_session.messages[offset + 1],
            session.messages[original_idx]
        );
    }

    // removed count: total - preserve_recent (no prior summary prefix)
    assert_eq!(
        result.removed_message_count,
        50 - config.preserve_recent_messages
    );
}

// --- LAVA P1: typed anchor (drift-free, supersede, faithfulness) ---

#[test]
fn anchor_fold_is_drift_free_across_fifty_rounds() {
    // Round 1 establishes facts with exact identifiers.
    let round1 = "<summary>\n\
        1. Primary Request and Intent: implement LAVA vault\n\
        3. Files and Code Sections:\n\
        - crates/core-types/src/session/mod.rs seal_evicted_to_vault\n\
        4. Errors and Fixes:\n\
        - ECONNREFUSED 127.0.0.1:5432 fixed by retry\n\
        6. All User Messages:\n\
        - please never lose context\n\
        </summary>";
    let mut anchor = fold_anchor(None, parse_anchor_from_summary(round1));
    // The fold carries `vault_ranges` forward verbatim; the per-round span is
    // appended by `apply_compaction` after the seal, which is simulated here
    // (width-10 contiguous spans) so accumulation is exercised end to end.
    assert!(
        anchor.vault_ranges.is_empty(),
        "fold from None carries the empty delta ranges"
    );
    anchor.vault_ranges.push((0, 9));

    // 50 further rounds each add a NEW unique fact; none may erode round 1.
    for round in 2..=50 {
        let delta = format!(
            "<summary>\n3. Files and Code Sections:\n- crates/runtime/src/round_{round}.rs\n</summary>"
        );
        let ranges_before = anchor.vault_ranges.len();
        anchor = fold_anchor(Some(&anchor), parse_anchor_from_summary(&delta));
        assert_eq!(
            anchor.vault_ranges.len(),
            ranges_before,
            "fold must not add or double-count a span (apply_compaction owns the push)"
        );
        let lo = (round - 1) * 10;
        anchor.vault_ranges.push((lo, lo + 9));
    }

    // Every round's span survives, in order, contiguous and non-overlapping.
    assert_eq!(anchor.vault_ranges.len(), 50, "one span per round accumulates");
    assert_eq!(anchor.vault_ranges[0], (0, 9), "round-1 span survives verbatim");
    assert_eq!(anchor.vault_ranges[49], (490, 499), "latest span present too");
    for pair in anchor.vault_ranges.windows(2) {
        assert_eq!(
            pair[1].0,
            pair[0].1 + 1,
            "spans are contiguous and never overlap across rounds"
        );
    }

    // Round-1 identifiers survive byte-identical — no summary-of-summary erosion.
    assert!(
        anchor
            .files
            .iter()
            .any(|file| file.contains("crates/core-types/src/session/mod.rs")),
        "round-1 file survives 50 rounds"
    );
    assert!(
        anchor
            .errors_and_fixes
            .iter()
            .any(|error| error.contains("ECONNREFUSED 127.0.0.1:5432")),
        "round-1 error survives verbatim"
    );
    assert!(
        anchor
            .user_messages
            .iter()
            .any(|message| message == "please never lose context"),
        "round-1 user message survives"
    );
    assert_eq!(anchor.intent, "implement LAVA vault", "intent stays stable");
    assert!(
        anchor.files.iter().any(|file| file.contains("round_50.rs")),
        "latest round's content is present too"
    );

    // The model-facing render does not drift either.
    let rendered = render_anchor_to_summary_text(&anchor);
    assert!(rendered.contains("crates/core-types/src/session/mod.rs"));
    assert!(rendered.contains("ECONNREFUSED 127.0.0.1:5432"));
}

#[test]
fn anchor_fold_keeps_distinct_facts_about_the_same_file() {
    // Two different code sections of the same file are BOTH facts — folding must
    // accumulate them, not let the later entry drop the earlier (the removed
    // supersede-by-path bug).
    let first = fold_anchor(
        None,
        parse_anchor_from_summary(
            "<summary>\n3. Files and Code Sections:\n- src/lib.rs parse_config reads ZO_HOME\n</summary>",
        ),
    );
    let second = fold_anchor(
        Some(&first),
        parse_anchor_from_summary(
            "<summary>\n3. Files and Code Sections:\n- src/lib.rs render_anchor emits sections\n</summary>",
        ),
    );

    assert!(
        second.files.iter().any(|file| file.contains("parse_config")),
        "earlier fact about src/lib.rs must not be dropped"
    );
    assert!(
        second.files.iter().any(|file| file.contains("render_anchor")),
        "later fact about src/lib.rs is added alongside"
    );
}

/// The `fold_anchor` union grows the accumulating sections without bound; over a
/// long-lived session the running summary swelled to ~150k tokens (60% of a 258k
/// GPT window), so compaction could only reclaim the small compactable tail and
/// re-fired within a few turns. `bound_anchor` caps each accumulating section to
/// its most recent content, keeping newest entries and dropping oldest (which
/// stay recoverable via the vault).
#[test]
fn bound_anchor_caps_accumulating_sections_keeping_recent() {
    use crate::session::AnchorSummary;

    // A section bloated far past budget (each entry ~120 chars → well over the
    // per-section budget), plus load-bearing snapshot fields that must survive.
    let bloated: Vec<String> = (0..2_000)
        .map(|i| format!("problem_solving step {i:04}: {}", "detail ".repeat(15)))
        .collect();
    let mut anchor = AnchorSummary {
        intent: "keep the context bounded".to_string(),
        problem_solving: bloated,
        pending_tasks: vec!["finish the reaper fix".to_string()],
        current_work: "bounding the anchor".to_string(),
        vault_ranges: vec![(0, 9), (10, 19)],
        ..AnchorSummary::default()
    };

    let before = render_anchor_to_summary_text(&anchor).len();
    bound_anchor(&mut anchor);
    let after = render_anchor_to_summary_text(&anchor).len();

    // The bloated section is now within budget (plus its single header line),
    // a large reduction from the pre-bound size.
    assert!(after < before, "bound must shrink a bloated anchor: {after} !< {before}");
    let section_chars: usize = anchor
        .problem_solving
        .iter()
        .map(|item| item.len() + "- \n".len())
        .sum();
    assert!(
        section_chars <= ANCHOR_SECTION_MAX_CHARS,
        "section retained {section_chars} chars, over budget {ANCHOR_SECTION_MAX_CHARS}"
    );

    // Newest entries are kept, oldest are evicted.
    assert!(
        anchor.problem_solving.iter().any(|s| s.contains("step 1999")),
        "the most recent entry must be retained"
    );
    assert!(
        !anchor.problem_solving.iter().any(|s| s.contains("step 0000")),
        "the oldest entry must be evicted"
    );

    // Protected fields are left completely intact.
    assert_eq!(anchor.intent, "keep the context bounded");
    assert_eq!(anchor.current_work, "bounding the anchor");
    assert_eq!(anchor.pending_tasks, vec!["finish the reaper fix".to_string()]);
    assert_eq!(anchor.vault_ranges, vec![(0, 9), (10, 19)]);
}

/// `pending_tasks` was the ONE anchor section with no bound at all, so it was
/// the one part of the running summary that grew forever — at index 0 of every
/// request, and self-accelerating, since a bigger anchor raises the
/// post-compaction floor and brings the next round sooner.
#[test]
fn bound_anchor_caps_pending_tasks_on_its_own_larger_budget() {
    use crate::session::AnchorSummary;

    let mut anchor = AnchorSummary {
        pending_tasks: (0..4_000)
            .map(|i| format!("task {i:04}: {}", "detail ".repeat(15)))
            .collect(),
        ..AnchorSummary::default()
    };
    bound_anchor(&mut anchor);

    let section_chars: usize = anchor
        .pending_tasks
        .iter()
        .map(|item| item.len() + "- \n".len())
        .sum();
    assert!(
        section_chars <= PENDING_TASKS_MAX_CHARS,
        "pending tasks retained {section_chars} chars, over budget {PENDING_TASKS_MAX_CHARS}"
    );
    assert!(
        section_chars > ANCHOR_SECTION_MAX_CHARS,
        "a task list gets the most room the anchor can afford, not the ordinary budget"
    );
    assert!(anchor.pending_tasks.iter().any(|s| s.contains("task 3999")));
    assert!(!anchor.pending_tasks.iter().any(|s| s.contains("task 0000")));
}

/// The ordering that makes bounding `pending_tasks` safe: a round that restates
/// a task moves it to the back, because restatement is live evidence the task
/// is still open.
///
/// Under first-seen ordering this was inverted — a task restated every single
/// round kept its original position at the FRONT and would be the first
/// evicted, while a task nothing had mentioned in twenty rounds outlived it.
#[test]
fn a_restated_pending_task_moves_to_the_back_of_the_queue() {
    use crate::session::AnchorSummary;

    let prior = AnchorSummary {
        pending_tasks: vec![
            "still open: wire the reaper".to_string(),
            "probably done: rename the flag".to_string(),
        ],
        ..AnchorSummary::default()
    };
    // This round looked and saw only the first task still pending.
    let delta = AnchorSummary {
        pending_tasks: vec!["still open: wire the reaper".to_string()],
        ..AnchorSummary::default()
    };

    let folded = fold_anchor(Some(&prior), delta);

    assert_eq!(
        folded.pending_tasks,
        vec![
            "probably done: rename the flag".to_string(),
            "still open: wire the reaper".to_string(),
        ],
        "the task this round confirmed is newest; the unmentioned one is the eviction candidate"
    );
}

/// Refreshing must not lose a task, only reorder it: a round that mentions
/// nothing leaves the queue exactly as it was.
#[test]
fn folding_never_drops_a_pending_task_it_did_not_see() {
    use crate::session::AnchorSummary;

    let prior = AnchorSummary {
        pending_tasks: vec!["a".to_string(), "b".to_string()],
        ..AnchorSummary::default()
    };
    let folded = fold_anchor(Some(&prior), AnchorSummary::default());
    assert_eq!(folded.pending_tasks, vec!["a".to_string(), "b".to_string()]);

    // And a genuinely new task is appended rather than replacing the list.
    let with_new = fold_anchor(
        Some(&folded),
        AnchorSummary {
            pending_tasks: vec!["c".to_string()],
            ..AnchorSummary::default()
        },
    );
    assert_eq!(
        with_new.pending_tasks,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

/// The reminder set is re-appended whenever the live set falls out of the
/// dedupe window, so the nag stays near the tail. Keeping the NEWEST copy near
/// the tail is the deliberate part; nothing wanted the older ones, and nothing
/// could reclaim them — microcompact cleared only tool results and images, so
/// this mass was structurally invisible to it and only a full compaction round
/// could evict it. Real sessions carry tens of thousands of tokens of it (one
/// repeated a single 2,170-byte reminder seventeen times).
#[test]
fn microcompact_clears_a_reminder_a_later_identical_copy_superseded() {
    let reminder = format!("<system-reminder>\n{}\n</system-reminder>", "nag ".repeat(400));
    let mut session = Session::new();
    session.push_user_text("start").expect("message");
    for _ in 0..3 {
        session
            .push_message(reminder_message(&reminder))
            .expect("reminder");
        session.push_user_text("work").expect("message");
    }

    let event = microcompact_session(&mut session, 0, 1).expect("superseded copies to clear");
    assert_eq!(
        event.cleared_superseded_reminders, 2,
        "the two older copies go; the newest stays"
    );
    assert!(event.estimated_tokens_saved > 0);

    let live: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::System)
        .filter_map(|m| match m.blocks.first() {
            Some(ContentBlock::Text { text }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        live,
        vec![
            MICROCOMPACT_REMINDER_PLACEHOLDER,
            MICROCOMPACT_REMINDER_PLACEHOLDER,
            reminder.as_str(),
        ],
        "the surviving copy must be the newest one — the nag stays near the tail"
    );

    // Idempotent: a placeholder is not itself a reminder, so a second pass
    // finds nothing left to clear.
    assert!(
        microcompact_session(&mut session, 0, 1).is_none(),
        "a cleared reminder must not be a candidate again"
    );
}

/// A reminder whose TEXT changed is a different reminder, not a duplicate of
/// the version it replaced — only an exact byte match is superseded.
#[test]
fn microcompact_keeps_reminders_that_differ_even_slightly() {
    let mut session = Session::new();
    session.push_user_text("start").expect("message");
    session
        .push_message(reminder_message(
            "<system-reminder>todo: step 1 in progress</system-reminder>",
        ))
        .expect("reminder");
    session.push_user_text("work").expect("message");
    session
        .push_message(reminder_message(
            "<system-reminder>todo: step 2 in progress</system-reminder>",
        ))
        .expect("reminder");

    assert!(
        microcompact_session(&mut session, 0, 1).is_none(),
        "two different reminders are not copies of each other"
    );
}

/// A compaction continuation also rides a `System` message. It carries no
/// `<system-reminder>` tag, so it can never be mistaken for a reminder — which
/// matters, because clearing it would erase the entire summarized history.
#[test]
fn microcompact_never_touches_a_compaction_continuation() {
    let continuation = get_compact_continuation_message("prior summary", true, true, &[]);
    let mut session = Session::new();
    session
        .push_message(reminder_message(&continuation))
        .expect("continuation");
    session.push_user_text("work").expect("message");
    session
        .push_message(reminder_message(&continuation))
        .expect("continuation again");

    assert!(
        microcompact_session(&mut session, 0, 1).is_none(),
        "a summary is not a reminder, even when an identical one appears twice"
    );
}

/// FRONTIER DISCIPLINE. A microcompact firing costs one re-bill of the wire
/// prefix from its EARLIEST rewritten message to the tail, so that index is the
/// whole price of the pass. The reminder scan has no `keep_recent` and used to
/// run over the whole transcript, which meant a single duplicate reminder near
/// message 1 could set that index at the head of the conversation and re-bill
/// nearly everything to reclaim a few hundred bytes. It now reclaims only what
/// sits at or after the frontier the tool-result pass already establishes —
/// where it is free.
#[test]
fn a_reminder_at_or_after_the_tool_frontier_is_reclaimed_in_the_same_pass() {
    let reminder = format!("<system-reminder>\n{}\n</system-reminder>", "nag ".repeat(400));
    let big = "x".repeat(1_000);
    let mut messages = vec![reminder_message(&reminder)];
    messages.extend((0..8).map(|i| tool_result_message(i, &big, 0)));
    messages.push(reminder_message(&reminder));
    messages.push(reminder_message(&reminder));
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(messages);

    // 8 results, keep-recent 5 -> the three oldest clear, so the frontier is
    // message 1. Copy 0 sits behind it; copy 9 sits inside the re-billed region;
    // copy 10 is the newest and is always kept.
    let event = microcompact_session(&mut session, 5, 240).expect("three old results clearable");
    assert_eq!(event.cleared_results, 3);
    assert_eq!(
        event.cleared_superseded_reminders, 1,
        "only the copy at or after the frontier is reclaimed"
    );

    let text_at = |index: usize| match &session.messages[index].blocks[0] {
        ContentBlock::Text { text } => text.clone(),
        other => panic!("expected text at {index}: {other:?}"),
    };
    assert_eq!(
        text_at(0),
        reminder,
        "a copy ahead of the frontier is left in place — reclaiming it would \
         move the divergence index to the head of the transcript"
    );
    assert_eq!(text_at(9), MICROCOMPACT_REMINDER_PLACEHOLDER);
    assert_eq!(text_at(10), reminder, "the newest copy always survives");
}

/// The frontier only ever moves FORWARD (a cleared body is never a candidate
/// again), so the copy left behind it is not picked up by a later pass either.
/// That is the deliberate side of the trade: it keeps costing its own bytes
/// until full compaction evicts it, which is far cheaper than one whole-context
/// re-bill.
#[test]
fn a_reminder_stranded_behind_the_frontier_is_not_reclaimed_by_a_later_pass() {
    let reminder = format!("<system-reminder>\n{}\n</system-reminder>", "nag ".repeat(400));
    let big = "x".repeat(1_000);
    let mut messages = vec![reminder_message(&reminder)];
    messages.extend((0..8).map(|i| tool_result_message(i, &big, 0)));
    messages.push(reminder_message(&reminder));
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(messages);

    microcompact_session(&mut session, 5, 240).expect("first pass clears the old results");
    // Second pass: the three oldest results are placeholders now, so the
    // frontier has advanced past message 0 for good.
    assert!(
        microcompact_session(&mut session, 4, 240).is_some(),
        "a smaller keep budget still finds results to clear"
    );
    match &session.messages[0].blocks[0] {
        ContentBlock::Text { text } => assert_eq!(*text, reminder),
        other => panic!("expected the stranded reminder: {other:?}"),
    }
}

/// A session whose clearable results have not yet outgrown `keep_recent` has no
/// frontier at all. The reminder pass is then the whole pass and sets the
/// divergence index itself, so it stays enabled — dropping it here would switch
/// reclamation off in exactly the long, low-tool-traffic session it was written
/// for. What prices it is the caller's break-even gate, which is calibrated for
/// a whole-prefix rewrite.
#[test]
fn with_no_tool_frontier_the_reminder_pass_still_runs() {
    let reminder = format!("<system-reminder>\n{}\n</system-reminder>", "nag ".repeat(400));
    let big = "x".repeat(1_000);
    let mut messages: Vec<ConversationMessage> =
        (0..3).map(|i| tool_result_message(i, &big, 0)).collect();
    messages.push(reminder_message(&reminder));
    messages.push(reminder_message(&reminder));
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(messages);

    let event = microcompact_session(&mut session, 5, 240)
        .expect("no tool result clears, but the reminders still do");
    assert_eq!(event.cleared_results, 0, "3 results are inside the keep budget");
    assert_eq!(event.cleared_superseded_reminders, 1);
}

/// `mutation_frontier` folds BOTH clear sets, and the image set is not a
/// redundant second source: a long session can be pinned by pasted screenshots
/// with no clearable tool result in sight (that is the case
/// `microcompact_clears_old_standalone_images_but_keeps_recent` covers). If the
/// frontier were derived from tool results alone it would read `None` here, the
/// `is_none_or` filter would open the scan to the whole transcript, and the copy
/// this test strands would be reclaimed — dragging the divergence index to
/// message 0 while the images being cleared had already priced message 1.
#[test]
fn an_images_only_pass_still_sets_the_reminder_frontier() {
    let reminder = format!("<system-reminder>\n{}\n</system-reminder>", "nag ".repeat(400));
    let image_message = |i: usize| ConversationMessage {
        role: MessageRole::User,
        blocks: vec![
            ContentBlock::Text {
                text: format!("look at screenshot {i}"),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
            },
        ],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    };
    let mut messages = vec![reminder_message(&reminder)];
    messages.extend((0..7).map(image_message));
    messages.push(reminder_message(&reminder));
    messages.push(reminder_message(&reminder));
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(messages);

    // 7 images, keep-recent 5 -> the two oldest clear, at messages 1 and 2, so
    // the frontier is message 1 and NO tool result contributed to it.
    let event = microcompact_session(&mut session, 5, 240).expect("two old images clearable");
    assert_eq!(event.cleared_results, 0, "there is no tool result in this session");
    assert_eq!(event.cleared_images, 2);
    assert_eq!(
        event.cleared_superseded_reminders, 1,
        "only the copy at or after the image frontier is reclaimed"
    );

    let text_at = |index: usize| match &session.messages[index].blocks[0] {
        ContentBlock::Text { text } => text.clone(),
        other => panic!("expected text at {index}: {other:?}"),
    };
    assert_eq!(
        text_at(0),
        reminder,
        "the copy ahead of the image frontier is left in place"
    );
    assert_eq!(text_at(8), MICROCOMPACT_REMINDER_PLACEHOLDER);
    assert_eq!(text_at(9), reminder, "the newest copy always survives");
}

fn reminder_message(text: &str) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }
}

/// A single entry larger than the whole budget is kept rather than dropped:
/// losing the newest entry loses more than a slightly over-budget section, and
/// truncating mid-entry would corrupt an identifier.
#[test]
fn bound_anchor_keeps_a_single_oversized_recent_entry() {
    use crate::session::AnchorSummary;

    let huge = "x".repeat(ANCHOR_SECTION_MAX_CHARS * 2);
    let mut anchor = AnchorSummary {
        files: vec!["old/small.rs".to_string(), huge.clone()],
        ..AnchorSummary::default()
    };

    bound_anchor(&mut anchor);

    assert_eq!(anchor.files, vec![huge], "newest entry survives; older one drops");
}

/// A healthy session under budget is untouched — the bound only trims pathological
/// growth, so normal compaction keeps carrying every entry forward verbatim.
#[test]
fn bound_anchor_leaves_a_small_anchor_unchanged() {
    use crate::session::AnchorSummary;

    let mut anchor = AnchorSummary {
        concepts: vec!["typed anchor".to_string(), "vault recall".to_string()],
        files: vec!["src/lib.rs".to_string()],
        user_messages: vec!["please never lose context".to_string()],
        ..AnchorSummary::default()
    };
    let expected = anchor.clone();

    bound_anchor(&mut anchor);

    assert_eq!(anchor, expected, "a within-budget anchor is carried forward verbatim");
}

/// The span the continuation advertises must be reachable by the call it writes.
///
/// `format_vault_recall_affordance` hands the model a ready-to-paste
/// `{"session_ref": "current", "seq_from": X, "seq_to": Y}`. The test above
/// checks the sentence is *shaped* right, with the ranges passed in as
/// literals; `session_recall`'s own vault tests recall with no seq window at
/// all. Neither asks the question that decides whether the model actually gets
/// its pre-compaction original back: **is every seq in `X..=Y` below the
/// post-compaction `first_message_index`?**
///
/// It has to be. `session_recall` treats a vault record at or above that index
/// as a microcompact pre-clear original of a still-live turn — a heal source,
/// not an evicted message — so it never lists it. A compaction that sealed one
/// message more than it rebased past would advertise a span whose last entry
/// silently returns nothing, and every end would still be green.
#[test]
fn every_seq_the_continuation_advertises_reads_as_evicted() {
    // Serialized on the crate env lock: a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, under which no span is sealed and the
    // affordance this test reads is absent (red once in the full gate,
    // 2026-09-22, green alone three times).
    let _env_guard = crate::test_env_lock();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-vault-span-{unique}"));
    std::fs::create_dir_all(&dir).expect("mk temp dir");
    let path = dir.join("sess.jsonl");

    let mut session = Session::new().with_persistence_path(path.clone());
    for i in 0..10 {
        session
            .push_user_text(format!("VAULTED_TURN_{i} some request text"))
            .expect("push user");
        session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: format!("answer {i}"),
            }]))
            .expect("push assistant");
    }

    let result = compact_session(
        &session,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        },
    );
    assert!(result.removed_message_count > 0, "the session compacted");

    // Read the span straight out of the sentence the model is shown.
    let continuation = result
        .compacted_session
        .messages
        .first()
        .map(|message| {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .expect("the compacted session opens with the continuation");
    let seq_from = advertised_seq("\"seq_from\":", &continuation);
    let seq_to = advertised_seq("\"seq_to\":", &continuation);
    assert!(seq_from <= seq_to, "the advertised span is ordered");

    let first_live = result.compacted_session.first_message_index();
    // Tight on purpose. Either direction is a real defect:
    //   seq_to >= first_live  — the tail of the advertised span reads as a
    //                           still-live turn's pre-clear original, so the
    //                           model asks for it and gets nothing back;
    //   seq_to <  first_live-1 — a message left the live transcript without a
    //                           vault record, which is the loss the vault exists
    //                           to prevent.
    assert_eq!(
        u64::from(seq_to) + 1,
        u64::from(first_live),
        "the advertised span (seq {seq_from}-{seq_to}) must end exactly where the live \
         transcript begins ({first_live})"
    );

    // Round two. The fold carries prior spans forward, so the advertised
    // `seq_to` has to track the NEW boundary — a second round that keeps
    // advertising the first round's end would strand everything it just sealed.
    let mut again = result.compacted_session.clone();
    for i in 10..20 {
        again
            .push_user_text(format!("VAULTED_TURN_{i} some request text"))
            .expect("push user");
        again
            .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
                text: format!("answer {i}"),
            }]))
            .expect("push assistant");
    }
    let second = compact_session(
        &again,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        },
    );
    assert!(second.removed_message_count > 0, "the second round compacted");
    let continuation = second
        .compacted_session
        .messages
        .first()
        .map(|message| {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .expect("the second continuation");
    let seq_to_2 = advertised_seq("\"seq_to\":", &continuation);
    let first_live_2 = second.compacted_session.first_message_index();
    assert_eq!(
        u64::from(seq_to_2) + 1,
        u64::from(first_live_2),
        "round two must advertise its own boundary, not round one's"
    );
    assert!(
        seq_to_2 > seq_to,
        "round two sealed more, so its advertised end must move: {seq_to} -> {seq_to_2}"
    );
}

/// A `"seq_from"`/`"seq_to"` value read out of the affordance sentence — the
/// model parses this same text, so the test reads it the same way.
fn advertised_seq(name: &str, continuation: &str) -> u32 {
    let at = continuation
        .find(name)
        .unwrap_or_else(|| panic!("{name} in the affordance: {continuation}"))
        + name.len();
    continuation[at..]
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or_else(|_| panic!("{name} is a number: {continuation}"))
}

#[test]
fn continuation_vault_affordance_is_emitted_and_excluded_from_legacy_reparse() {
    let summary = "<summary>\n\
        1. Primary Request and Intent: keep the context lossless\n\
        3. Files and Code Sections:\n\
        - src/lib.rs main entry\n\
        </summary>";
    let ranges = [(0u32, 9u32), (10, 19)];
    let continuation = get_compact_continuation_message(summary, true, true, &ranges);

    // The affordance line is present, names the exact spans, and is a directly
    // usable `session_recall` call.
    assert!(
        continuation.contains("preserved in this session's vault (seq 0-9, 10-19)"),
        "{continuation}"
    );
    assert!(continuation.contains("session_recall"), "{continuation}");
    assert!(continuation.contains("\"session_ref\": \"current\""), "{continuation}");
    assert!(continuation.contains("\"seq_from\": 0"), "{continuation}");
    assert!(continuation.contains("\"seq_to\": 19"), "{continuation}");

    // The legacy prose recovery path — extract the continuation body, then parse
    // it into an anchor — must NOT fold the affordance line into anchor content
    // (it rides outside the summary body, after the resume instruction).
    let system_message = ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text {
            text: continuation.clone(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    };
    let extracted = super::extract_existing_compacted_summary(&system_message)
        .expect("continuation is recognized as a compacted summary");
    assert!(
        !extracted.contains("session_recall") && !extracted.contains("seq_from"),
        "affordance is excluded from the extracted summary body: {extracted}"
    );

    let anchor = parse_anchor_from_summary(&extracted);
    let anchor_text = format!(
        "{}\n{}\n{}",
        anchor.intent,
        anchor.current_work,
        anchor
            .concepts
            .iter()
            .chain(anchor.files.iter())
            .chain(anchor.errors_and_fixes.iter())
            .chain(anchor.problem_solving.iter())
            .chain(anchor.user_messages.iter())
            .chain(anchor.pending_tasks.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
    );
    assert!(
        !anchor_text.contains("session_recall"),
        "reparsed anchor is not polluted by the affordance: {anchor_text}"
    );
    assert!(
        anchor.files.iter().any(|file| file.contains("src/lib.rs main entry")),
        "the original summary content still round-trips: {anchor_text}"
    );
}

#[test]
fn microcompact_cleared_body_is_restored_to_vault_on_compaction() {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, which would make this vault look legitimately
    // empty; the var is process-global, so exclusion is the only defense.
    let _env_guard = crate::test_env_lock();

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-lava-restore-{unique}"));
    std::fs::create_dir_all(&dir).expect("mk temp dir");
    let path = dir.join("sess.jsonl");

    // Persist a transcript whose big READ tool-result body is verbatim on disk.
    // (READ is not an edit tool, so microcompact will clear it.)
    let original_body = format!("ORIGINAL_TOOL_OUTPUT {}", "x".repeat(300));
    let mut session = Session::new().with_persistence_path(path.clone());
    session.push_user_text("start the task").expect("push");
    session
        .push_message(ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-restore-1".to_string(),
                tool_name: "Read".to_string(),
                output: original_body.clone(),
                is_error: false,
                images: Vec::new(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        })
        .expect("push tool result");
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "did some work".to_string(),
        }]))
        .expect("push");
    session.push_user_text("keep going one").expect("push");
    session.push_user_text("keep going two").expect("push");
    session.push_user_text("recent tail a").expect("push");
    session.push_user_text("recent tail b").expect("push");

    // Microcompact clears the OLD tool-result body in memory. This test does not
    // persist afterwards, so the on-disk snapshot still holds the original — the
    // narrow window the disk FALLBACK covers. (In production the clear marks the
    // transcript dirty and the next persist publishes the placeholder; that
    // ordering is covered by
    // `a_cleared_result_is_recovered_from_the_vault_after_the_clear_is_persisted`.)
    let event = microcompact_session(&mut session, 0, 10).expect("microcompact clears the body");
    assert_eq!(event.cleared_results, 1);
    if let ContentBlock::ToolResult { output, .. } = &session.messages[1].blocks[0] {
        assert_eq!(output, MICROCOMPACT_PLACEHOLDER, "in-memory body is now the placeholder");
    } else {
        panic!("expected a tool result at messages[1].blocks[0]");
    }

    // Full compaction evicts the older messages. The restore step must seal the
    // ORIGINAL body (recovered from disk) to the vault, not the placeholder.
    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 0,
    };
    let result = compact_session(&session, config);
    assert!(result.removed_message_count > 0, "compaction evicted older messages");

    let vault = result.compacted_session.read_vault();
    let restored = vault.iter().any(|record| {
        record.message.blocks.iter().any(
            |block| matches!(block, ContentBlock::ToolResult { output, .. } if *output == original_body),
        )
    });
    assert!(
        restored,
        "microcompact-cleared body is restored to its original in the vault"
    );
    let sealed_placeholder = vault.iter().any(|record| {
        record.message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::ToolResult { output, .. } if output.as_str() == MICROCOMPACT_PLACEHOLDER)
        })
    });
    assert!(!sealed_placeholder, "the placeholder must NOT have been sealed to the vault");

    let _ = std::fs::remove_dir_all(&dir);
}

// --- LAVA: microcompact seals its pre-clear originals ---

/// A fresh temp directory plus the session path inside it, named after the
/// caller so two tests never share a vault file.
fn temp_session_path(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-lava-{tag}-{unique}"));
    std::fs::create_dir_all(&dir).expect("mk temp dir");
    let path = dir.join("sess.jsonl");
    (dir, path)
}

/// Raw (non-deduplicated) vault lines, so a test can tell "sealed twice" from
/// "sealed once" — `Session::read_vault` collapses duplicate seqs by design and
/// therefore cannot see a double-seal.
fn raw_vault_lines(session_path: &std::path::Path) -> Vec<String> {
    let vault = session_path.with_file_name("sess.vault.jsonl");
    std::fs::read_to_string(vault).map_or_else(
        |_| Vec::new(),
        |text| text.lines().filter(|line| !line.trim().is_empty()).map(str::to_string).collect(),
    )
}

/// Every tool-result body present in the session's vault, after dedup.
fn vault_tool_bodies(session: &Session) -> Vec<String> {
    session
        .read_vault()
        .into_iter()
        .flat_map(|record| record.message.blocks)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { output, .. } => Some(output),
            _ => None,
        })
        .collect()
}

/// Build a persisted session whose message 1 carries one big, clearable
/// (non-edit) tool result, plus enough tail to keep compaction realistic.
fn persisted_session_with_one_big_result(path: &std::path::Path, body: &str) -> Session {
    let mut session = Session::new().with_persistence_path(path.to_path_buf());
    session.push_user_text("start the task").expect("push");
    session
        .push_message(tool_result_message(1, body, 0))
        .expect("push tool result");
    session
        .push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "did some work".to_string(),
        }]))
        .expect("push");
    session.push_user_text("recent tail a").expect("push");
    session.push_user_text("recent tail b").expect("push");
    session
}

#[test]
fn a_cleared_result_is_sealed_before_it_is_overwritten() {
    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, which would make this vault look legitimately
    // empty; the var is process-global, so exclusion is the only defense.
    let _env_guard = crate::test_env_lock();
    let (dir, path) = temp_session_path("seal-before-clear");
    let original_body = format!("ORIGINAL_TOOL_OUTPUT {}", "x".repeat(300));
    let mut session = persisted_session_with_one_big_result(&path, &original_body);

    let event = microcompact_session(&mut session, 0, 10).expect("microcompact clears the body");
    assert_eq!(event.cleared_results, 1);

    // In memory the body is gone — that is the whole point of the trim.
    match session.messages[1].blocks.first() {
        Some(ContentBlock::ToolResult { output, .. }) => {
            assert_eq!(output, MICROCOMPACT_PLACEHOLDER, "live body is the placeholder");
        }
        other => panic!("unexpected block: {other:?}"),
    }
    // The vault holds the ORIGINAL, captured one statement before the overwrite.
    let bodies = vault_tool_bodies(&session);
    assert!(
        bodies.iter().any(|body| body == &original_body),
        "vault holds the pre-clear original, got {bodies:?}"
    );
    assert!(
        !bodies.iter().any(|body| body == MICROCOMPACT_PLACEHOLDER),
        "the placeholder must never be what the vault stores"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cleared_result_is_recovered_from_the_vault_after_the_clear_is_persisted() {
    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, which would make this vault look legitimately
    // empty; the var is process-global, so exclusion is the only defense.
    let _env_guard = crate::test_env_lock();
    // Production ordering: the clear marks the transcript dirty, so the NEXT
    // persist rewrites the on-disk snapshot with the placeholder — long before
    // full compaction runs. That is what made the disk-only restore structurally
    // incapable of recovering anything.
    let (dir, path) = temp_session_path("disk-poisoned");
    let original_body = format!("ORIGINAL_TOOL_OUTPUT {}", "x".repeat(300));
    let mut session = persisted_session_with_one_big_result(&path, &original_body);
    let _ = microcompact_session(&mut session, 0, 10).expect("microcompact clears the body");
    session.save_to_path(&path).expect("persist the cleared snapshot");

    // Assert the fallback source really is poisoned, so the recovery below can
    // only have come from the vault.
    let on_disk = std::fs::read_to_string(&path).expect("read snapshot");
    assert!(
        on_disk.contains(MICROCOMPACT_PLACEHOLDER),
        "the persisted snapshot holds the placeholder"
    );
    assert!(
        !on_disk.contains("ORIGINAL_TOOL_OUTPUT"),
        "the persisted snapshot no longer holds the original"
    );

    let mut evicted = session.messages[..2].to_vec();
    // Live indices 0 and 1 with `first_message_index == 0`, so those are also the
    // absolute seqs (only the image half reads them; this case is id-addressed).
    super::restore_microcompacted_bodies(&mut evicted, &session, &[0, 1]);
    match evicted[1].blocks.first() {
        Some(ContentBlock::ToolResult { output, .. }) => {
            assert_eq!(output, &original_body, "restored from the vault, not from disk");
        }
        other => panic!("unexpected block: {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn re_running_the_clear_does_not_double_seal() {
    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, which would make this vault look legitimately
    // empty; the var is process-global, so exclusion is the only defense.
    let _env_guard = crate::test_env_lock();
    let (dir, path) = temp_session_path("idempotent-seal");
    let original_body = format!("ORIGINAL_TOOL_OUTPUT {}", "x".repeat(300));
    let mut session = persisted_session_with_one_big_result(&path, &original_body);

    let _ = microcompact_session(&mut session, 0, 10).expect("first pass clears the body");
    let after_first = raw_vault_lines(&path);
    assert_eq!(after_first.len(), 1, "one message cleared, one record sealed");

    let second = microcompact_session(&mut session, 0, 10);
    assert!(second.is_none(), "nothing new to clear on the second pass");
    assert_eq!(
        raw_vault_lines(&path),
        after_first,
        "a no-op pass must append nothing to the append-only vault"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_second_clear_in_the_same_message_reseals_both_originals() {
    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, which would make this vault look legitimately
    // empty; the var is process-global, so exclusion is the only defense.
    let _env_guard = crate::test_env_lock();
    // The seq domain is per MESSAGE and the vault reader is last-wins, so a
    // later round re-sealing a message it had already sealed would bury the
    // pristine record under a partially-cleared one. The seal heals from the
    // vault first, so what lands is pristine both times.
    let (dir, path) = temp_session_path("reseal-heals");
    let first_body = format!("FIRST_BODY {}", "a".repeat(300));
    let second_body = format!("SECOND_BODY {}", "b".repeat(300));
    let mut session = Session::new().with_persistence_path(path.clone());
    session
        .push_message(ConversationMessage {
            role: MessageRole::User,
            blocks: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "pair-a".to_string(),
                    tool_name: "Read".to_string(),
                    output: first_body.clone(),
                    is_error: false,
                    images: Vec::new(),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "pair-b".to_string(),
                    tool_name: "Read".to_string(),
                    output: second_body.clone(),
                    is_error: false,
                    images: Vec::new(),
                },
            ],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        })
        .expect("push paired results");
    session
        .push_message(tool_result_message(9, &"z".repeat(300), 0))
        .expect("push filler result");
    session
        .push_message(tool_result_message(10, &"z".repeat(300), 0))
        .expect("push filler result");

    // Four clearable results; keep 3 clears exactly the first block, keep 2 then
    // clears exactly the second — two rounds touching the SAME message.
    let first_pass = microcompact_session(&mut session, 3, 240).expect("first block clearable");
    assert_eq!(first_pass.cleared_results, 1);
    let second_pass = microcompact_session(&mut session, 2, 240).expect("second block clearable");
    assert_eq!(second_pass.cleared_results, 1);

    let bodies = vault_tool_bodies(&session);
    assert!(
        bodies.iter().any(|body| body == &first_body),
        "round one's original survives round two's re-seal, got {bodies:?}"
    );
    assert!(
        bodies.iter().any(|body| body == &second_body),
        "round two's original is sealed too, got {bodies:?}"
    );
    assert!(
        !bodies.iter().any(|body| body == MICROCOMPACT_PLACEHOLDER),
        "no placeholder is ever the surviving vault record"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The live tool-result body of the fixture's message 1.
fn live_body(session: &Session) -> String {
    match session.messages[1].blocks.first() {
        Some(ContentBlock::ToolResult { output, .. }) => output.clone(),
        other => panic!("unexpected block: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_seal_that_cannot_be_written_leaves_the_body_intact() {
    use std::os::unix::fs::PermissionsExt;

    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT` — which would turn this failed append into a
    // legitimate no-op and let the trim fire. The var is process-global, so
    // mutual exclusion is the only defense.
    let _env_guard = crate::test_env_lock();

    // The invariant: a body is destroyed only once its original is recoverable.
    // A failed vault append means it is NOT recoverable — and the disk fallback
    // cannot stand in, because this pass ends with `mark_transcript_dirty`, so
    // the next persist publishes the placeholder over the snapshot the fallback
    // reads. The trim must therefore decline rather than clear.
    let (dir, path) = temp_session_path("seal-write-failure");
    let original_body = format!("ORIGINAL_TOOL_OUTPUT {}", "x".repeat(300));
    let mut session = persisted_session_with_one_big_result(&path, &original_body);

    // The smallest real seam for "the append fails": the vault lives beside the
    // session file, so a read-only session directory makes the create+append
    // fail with EACCES. Same seam the todo-store and agent-tools IO-failure
    // tests use.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555))
        .expect("make the session directory read-only");
    let probe = dir.join(".probe");
    if std::fs::write(&probe, b"x").is_ok() {
        // This uid can write a 0555 directory anyway (root): the EACCES under
        // test cannot occur, so there is nothing to assert.
        let _ = std::fs::remove_file(&probe);
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let declined = microcompact_session(&mut session, 0, 10);
    let body_after_failure = live_body(&session);
    let vault_after_failure = raw_vault_lines(&path);

    // Restore write access before asserting, so a failing assertion cannot leave
    // an undeletable directory behind.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
        .expect("restore session directory permissions");

    assert!(
        declined.is_none(),
        "a trim whose seal could not be written must report no firing, got {declined:?}"
    );
    assert_eq!(
        body_after_failure, original_body,
        "the raw body is the only surviving copy, so it must not have been cleared"
    );
    assert!(
        vault_after_failure.is_empty(),
        "nothing was sealed, got {vault_after_failure:?}"
    );

    // Other direction, same session: with the directory writable the identical
    // plan seals and DOES clear — so the assertion above is about the write
    // failure, not about this batch being unclearable.
    let fired = microcompact_session(&mut session, 0, 10).expect("the same plan now fires");
    assert_eq!(fired.cleared_results, 1);
    assert_eq!(
        live_body(&session),
        MICROCOMPACT_PLACEHOLDER,
        "a successful seal is what licenses the clear"
    );
    assert!(
        vault_tool_bodies(&session).iter().any(|body| body == &original_body),
        "the original reached the vault before the clear"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_trim_with_the_vault_disabled_still_clears_and_writes_nothing() {
    // The documented intentional-loss mode: the operator turned the vault off,
    // so "nothing was sealed" is not a failed write and must not block the trim.
    let _guard = crate::test_env_lock();
    let (dir, path) = temp_session_path("vault-disabled");
    let original_body = format!("ORIGINAL_TOOL_OUTPUT {}", "x".repeat(300));
    let mut session = persisted_session_with_one_big_result(&path, &original_body);

    std::env::set_var("ZO_DISABLE_RAW_VAULT", "1");
    let fired = microcompact_session(&mut session, 0, 10);
    std::env::remove_var("ZO_DISABLE_RAW_VAULT");

    assert!(fired.is_some(), "an opted-out vault must not disable the trim");
    assert_eq!(
        live_body(&session),
        MICROCOMPACT_PLACEHOLDER,
        "the body is cleared — the loss is what the operator asked for"
    );
    assert!(
        raw_vault_lines(&path).is_empty(),
        "no vault file is written when the vault is disabled"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unpersisted_trim_still_clears_and_has_no_vault() {
    // The other intentional-loss mode: with no persistence path there is no file
    // to write and no file to recover from, so refusing the trim would only mean
    // an in-memory session can never shrink.
    let original_body = format!("ORIGINAL_TOOL_OUTPUT {}", "x".repeat(300));
    let mut session = Session::new();
    session.push_user_text("start the task").expect("push");
    session
        .push_message(tool_result_message(1, &original_body, 0))
        .expect("push tool result");
    session.push_user_text("recent tail a").expect("push");
    session.push_user_text("recent tail b").expect("push");

    let fired = microcompact_session(&mut session, 0, 10).expect("an unpersisted trim still fires");
    assert_eq!(fired.cleared_results, 1);
    assert_eq!(live_body(&session), MICROCOMPACT_PLACEHOLDER, "the body is cleared");
    assert!(
        session.read_vault().is_empty(),
        "an unpersisted session has no vault to read"
    );
}

/// The recall lineage is the one reminder family whose cleared copy is NOT
/// byte-identical to a survivor, so what it gives up is real: the historical
/// rendering of that turn's recalled memory. This pins the accepted boundary so
/// the SCOPE note on `seal_microcompact_originals` cannot quietly become false in
/// either direction — if a later change starts sealing the lineage, this fails and
/// the note has to be rewritten with it.
#[test]
fn a_cleared_recall_lineage_reminder_is_not_sealed() {
    let _guard = crate::test_env_lock();
    let (dir, path) = temp_session_path("lineage-not-sealed");
    let recall_reminder = |body: &str| {
        reminder_message(&format!(
            "<system-reminder>\n{}{body}\n</system-reminder>",
            crate::memory::recall::RECALL_SECTION_HEADER
        ))
    };
    let mut session = Session::new().with_persistence_path(path.clone());
    session
        .push_message(recall_reminder("- [older-entry](m/older.md) — OLDER_LINEAGE_BYTES"))
        .expect("push older recall section");
    session.push_user_text("some work happened").expect("push");
    session
        .push_message(recall_reminder("- [newer-entry](m/newer.md) — NEWER_LINEAGE_BYTES"))
        .expect("push newer recall section");

    // No tool result and no image: the frontier is None, so the reminder pass is
    // the whole pass and the older section is in scope even at message 0.
    let event = microcompact_session(&mut session, 5, 240).expect("the older section clears");
    assert_eq!(event.cleared_superseded_reminders, 1);
    assert_eq!(event.cleared_results, 0);
    assert_eq!(event.cleared_images, 0);
    match &session.messages[0].blocks[0] {
        ContentBlock::Text { text } => {
            assert_eq!(*text, MICROCOMPACT_REMINDER_PLACEHOLDER, "older section cleared");
        }
        other => panic!("expected the cleared section: {other:?}"),
    }

    // The accepted loss: the older section's bytes reached no vault record.
    assert!(
        raw_vault_lines(&path).is_empty(),
        "a reminder-only plan seals nothing; got {:?}",
        raw_vault_lines(&path)
    );
    let vault_text = session
        .read_vault()
        .into_iter()
        .flat_map(|record| record.message.blocks)
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text),
            _ => None,
        })
        .collect::<String>();
    assert!(
        !vault_text.contains("OLDER_LINEAGE_BYTES"),
        "the cleared lineage bytes are given up, not sealed: {vault_text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// One user message holding a caption plus one standalone (pasted) image —
/// the exact shape microcompact blanks in place, so the blanked form differs
/// from this only in block 1.
fn pasted_image_message(caption: &str, data: &str) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![
            ContentBlock::Text {
                text: caption.to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: data.to_string(),
            },
        ],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }
}

/// Every standalone-image `data` payload present in the session's vault, after
/// dedup — i.e. what a reader could actually still recover.
fn vault_image_payloads(session: &Session) -> Vec<String> {
    session
        .read_vault()
        .into_iter()
        .flat_map(|record| record.message.blocks)
        .filter_map(|block| match block {
            ContentBlock::Image { data, .. } => Some(data),
            _ => None,
        })
        .collect()
}

#[test]
fn a_cleared_standalone_image_survives_the_full_compaction_reseal() {
    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, which would make this vault look legitimately
    // empty; the var is process-global, so exclusion is the only defense.
    let _env_guard = crate::test_env_lock();
    // The dominant loss path for a pasted image, and the reason the image heal
    // exists: microcompact seals the pre-clear original at seq N, then full
    // compaction evicts that same message — now carrying the placeholder — and
    // seals it AGAIN at the same seq. `read_vault` dedups last-record-wins, so
    // without a heal the second, lossy record buries the first and the image is
    // gone from every reader.
    let (dir, path) = temp_session_path("image-reseal");
    let mut session = Session::new().with_persistence_path(path.clone());
    session.push_user_text("start the task").expect("push");
    session
        .push_message(pasted_image_message("SCREENSHOT_CONTEXT", "OLDIMAGEBYTES"))
        .expect("push pasted image");
    session.push_user_text("some work happened").expect("push");
    session.push_user_text("recent tail a").expect("push");
    session.push_user_text("recent tail b").expect("push");

    let event = microcompact_session(&mut session, 0, 10).expect("the old image clears");
    assert_eq!(event.cleared_images, 1);
    assert_eq!(event.cleared_results, 0);
    assert!(
        vault_image_payloads(&session).iter().any(|data| data == "OLDIMAGEBYTES"),
        "precondition: the pre-clear seal really stored the image"
    );

    // Full compaction evicts messages 0..3, which includes the blanked image
    // message at live index 1 — the same seq its microcompact record occupies.
    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 0,
    };
    let result = compact_session(&session, config);
    assert!(result.removed_message_count >= 2, "the image message was evicted");

    let payloads = vault_image_payloads(&result.compacted_session);
    assert!(
        payloads.iter().any(|data| data == "OLDIMAGEBYTES"),
        "the sealed image original must survive the re-seal at its own seq, got {payloads:?}"
    );
    let sealed_placeholder = result.compacted_session.read_vault().iter().any(|record| {
        record.message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text == super::MICROCOMPACT_IMAGE_PLACEHOLDER)
        })
    });
    assert!(
        !sealed_placeholder,
        "the image placeholder must not be what the surviving vault record holds"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_second_image_cleared_in_the_same_message_does_not_bury_the_first() {
    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, which would make this vault look legitimately
    // empty; the var is process-global, so exclusion is the only defense.
    let _env_guard = crate::test_env_lock();
    // The image twin of `a_second_clear_in_the_same_message_reseals_both_originals`,
    // and the second instance of the burial class this pass fixes: a message
    // holding TWO pasted images is selected by two different rounds, and the seq
    // domain is per MESSAGE, so round two re-seals the same seq. Without an image
    // heal the round-two record carries image one already blanked, and last-wins
    // dedup drops the round-one record that still had it.
    let (dir, path) = temp_session_path("image-reseal-heals");
    let mut session = Session::new().with_persistence_path(path.clone());
    session
        .push_message(ConversationMessage {
            role: MessageRole::User,
            blocks: vec![
                ContentBlock::Text {
                    text: "two screenshots".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "FIRSTIMAGEBYTES".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "SECONDIMAGEBYTES".to_string(),
                },
            ],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        })
        .expect("push a message with two pasted images");
    session.push_user_text("some work happened").expect("push");

    // keep_recent=1 clears exactly the first image; keep_recent=0 then clears the
    // second — two rounds touching the SAME message, hence the same vault seq.
    let first = microcompact_session(&mut session, 1, 240).expect("first image clearable");
    assert_eq!(first.cleared_images, 1);
    let second = microcompact_session(&mut session, 0, 240).expect("second image clearable");
    assert_eq!(second.cleared_images, 1);

    let payloads = vault_image_payloads(&session);
    assert!(
        payloads.iter().any(|data| data == "FIRSTIMAGEBYTES"),
        "round one's image survives round two's re-seal, got {payloads:?}"
    );
    assert!(
        payloads.iter().any(|data| data == "SECONDIMAGEBYTES"),
        "round two's image is sealed too, got {payloads:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_image_seal_belonging_to_a_different_turn_is_refused() {
    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, which would make this vault look legitimately
    // empty; the var is process-global, so exclusion is the only defense.
    let _env_guard = crate::test_env_lock();
    // What the positional address can get wrong, and why every hit is verified:
    // the seq axis is off by one for records sealed before the session's first
    // full compaction, so a lookup can land on the neighbouring turn's record.
    // Pasting that record here would replace this turn's own caption with
    // someone else's. The structural check refuses it and the turn stays blanked.
    let (dir, path) = temp_session_path("image-wrong-owner");
    let session = Session::new().with_persistence_path(path.clone());
    assert!(
        session.seal_originals_to_vault(&[(
            0,
            pasted_image_message("NEIGHBOUR_CAPTION", "NEIGHBOURBYTES"),
        )]),
        "the fixture's vault seed must actually reach the vault"
    );
    let mut messages = vec![ConversationMessage {
        role: MessageRole::User,
        blocks: vec![
            ContentBlock::Text {
                text: "MY_OWN_CAPTION".to_string(),
            },
            ContentBlock::Text {
                text: super::MICROCOMPACT_IMAGE_PLACEHOLDER.to_string(),
            },
        ],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }];
    super::restore_microcompacted_bodies(&mut messages, &session, &[0]);

    assert_eq!(
        messages[0].blocks[0],
        ContentBlock::Text {
            text: "MY_OWN_CAPTION".to_string()
        },
        "the turn keeps its own text — the neighbour's record must not overwrite it"
    );
    assert_eq!(
        messages[0].blocks[1],
        ContentBlock::Text {
            text: super::MICROCOMPACT_IMAGE_PLACEHOLDER.to_string()
        },
        "and it stays blanked rather than borrowing the neighbour's image"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_image_seal_that_two_turns_could_own_heals_neither_of_them() {
    // Serialized on the crate env lock because a sibling test flips
    // `ZO_DISABLE_RAW_VAULT`, which would make this vault look legitimately
    // empty; the var is process-global, so exclusion is the only defense.
    let _env_guard = crate::test_env_lock();
    // A pasted image has no id, so the heal is positional — and the seq axis is
    // off by one for records sealed before the session's first full compaction.
    // Two BARE images (no caption) back to back are indistinguishable once
    // blanked, so no evidence can say whether the record at seq N belongs to the
    // turn at N or, under the drift, to the turn at N+1. Guessing would render
    // one turn's image as the other's; refusing leaves the placeholder, which is
    // the pre-heal behavior. This pins the refusal so a later "improvement" that
    // drops the ambiguity gate has to argue with a test.
    let (dir, path) = temp_session_path("image-ambiguous");
    let bare_image = |data: &str| ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: data.to_string(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    };
    let session = Session::new().with_persistence_path(path.clone());
    assert!(
        session.seal_originals_to_vault(&[
            (0, bare_image("ALPHABYTES")),
            (1, bare_image("BETABYTES")),
            (2, bare_image("GAMMABYTES")),
        ]),
        "the fixture's vault seed must actually reach the vault"
    );
    let blanked = || ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text {
            text: super::MICROCOMPACT_IMAGE_PLACEHOLDER.to_string(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    };
    let mut messages = vec![blanked(), blanked(), blanked()];
    super::restore_microcompacted_bodies(&mut messages, &session, &[0, 1, 2]);

    let restored = |message: &ConversationMessage| {
        matches!(message.blocks.first(), Some(ContentBlock::Image { .. }))
    };
    assert!(
        !restored(&messages[0]),
        "the record at seq 0 could equally be turn 1's under the +1 drift — refused"
    );
    assert!(
        !restored(&messages[1]),
        "same for the record at seq 1 against turn 2 — refused"
    );
    // The last turn of a run is the documented boundary: nothing in this batch
    // can claim its record under the drift, so the ambiguity gate is vacuous
    // there and the exact-seq hit is used.
    assert!(
        restored(&messages[2]),
        "the tail of the run has no rival claimant in the batch, so it heals"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verifier_rejects_summary_with_fabricated_identifiers() {
    let evicted = vec![ConversationMessage::user_text("we edited the real file today")];
    // Three paths that never appeared in the evicted source.
    let fabricated = "<summary>\n3. Files and Code Sections:\n\
        - ghost/one.rs\n- ghost/two.rs\n- ghost/three.rs\n</summary>";
    assert!(
        summary_fabricates_identifiers(fabricated, &evicted, None),
        "wholesale fabrication is rejected"
    );
}

#[test]
fn verifier_rejects_when_one_real_path_is_recycled_among_fabrications() {
    let evicted = vec![ConversationMessage::user_text("only src/real/file.rs was touched")];
    // One grounded path must not immunize three fabricated ones (majority rule).
    let mostly_fabricated = "<summary>\n3. Files and Code Sections:\n\
        - src/real/file.rs\n- ghost/one.rs\n- ghost/two.rs\n- ghost/three.rs\n</summary>";
    assert!(
        summary_fabricates_identifiers(mostly_fabricated, &evicted, None),
        "a single recycled real path does not immunize fabrications"
    );
}

#[test]
fn verifier_passes_faithful_summary() {
    let evicted = vec![ConversationMessage::user_text(
        "edited src/real/file.rs and lib/other.rs and bin/main.rs",
    )];
    let faithful = "<summary>\n3. Files and Code Sections:\n\
        - src/real/file.rs\n- lib/other.rs\n- bin/main.rs\n</summary>";
    assert!(
        !summary_fabricates_identifiers(faithful, &evicted, None),
        "a grounded summary passes the verifier"
    );
}

// --- re-compaction (typed-anchor fold) ---

#[test]
fn re_compaction_merges_summaries_without_losing_highlights() {
    // First compaction
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("edit rust/crates/runtime/src/compact.rs"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Done editing compact.rs.".to_string(),
        }]),
        ConversationMessage::user_text("now edit rust/crates/runtime/src/summary_compression.rs"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Pending: run clippy to verify.".to_string(),
        }]),
    ]);
    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
    };

    let first = compact_session(&session, config);
    assert_eq!(first.removed_message_count, 2);

    // Extend with new messages and re-compact
    let mut second_session = Session::new();
    second_session.messages = first.compacted_session.messages.clone();
    ::std::sync::Arc::make_mut(&mut second_session.messages).extend([
        ConversationMessage::user_text("add tests for edge cases"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Tests added. todo: verify coverage.".to_string(),
        }]),
    ]);

    let second = compact_session(&second_session, config);

    // LAVA P1: both rounds' content coexist in the folded typed anchor —
    // round-1 identifiers survive verbatim (no summary-of-summary erosion) and
    // round-2 content is folded in alongside.
    assert!(
        second.formatted_summary.contains("compact.rs"),
        "round-1 file reference must survive into round 2"
    );
    assert!(
        second.formatted_summary.contains("summary_compression.rs"),
        "round-2 file reference must be folded in"
    );

    let continuation_text = match &second.compacted_session.messages[0].blocks[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text block in system message"),
    };
    assert!(continuation_text.contains("compact.rs"));
    assert!(continuation_text.contains("summary_compression.rs"));
}

// --- tool context preservation ---

#[test]
fn compact_summary_includes_tool_names_from_removed_messages() {
    let mut session = Session::new();
    // Tool pairs 1 and 2 are compacted; pair 3 sits entirely in the preserved tail
    // so the tail does not begin with an orphan tool_result.
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("run some tools"),
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "1".to_string(),
            name: "bash".to_string(),
            input: r#"{"command":"cargo test"}"#.to_string(),
        }]),
        ConversationMessage::tool_result("1", "bash", "all tests passed", false),
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "2".to_string(),
            name: "read_file".to_string(),
            input: r#"{"path":"src/lib.rs"}"#.to_string(),
        }]),
        ConversationMessage::tool_result("2", "read_file", "file contents...", false),
        // preserved tail (2 messages — full tool_use/tool_result pair)
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "3".to_string(),
            name: "edit_file".to_string(),
            input: r#"{"path":"src/lib.rs"}"#.to_string(),
        }]),
        ConversationMessage::tool_result("3", "edit_file", "ok", false),
    ]);

    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
    };

    let result = compact_session(&session, config);

    // tool names from the removed portion must appear in the summary
    assert!(
        result.summary.contains("bash"),
        "summary should mention bash tool"
    );
    assert!(
        result.summary.contains("read_file"),
        "summary should mention read_file tool"
    );
}

#[test]
fn compact_preserved_tail_never_begins_with_orphan_tool_result() {
    // Regression: if the naive cut point lands on a user message whose first
    // block is a tool_result, the matching assistant tool_use gets summarized
    // away and the Anthropic API rejects the payload with
    // `unexpected tool_use_id in tool_result blocks`. The fix walks the cut
    // point backward past such boundaries.
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("start"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "ok".to_string(),
        }]),
        ConversationMessage::user_text("run bash"),
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "99".to_string(),
            name: "bash".to_string(),
            input: r#"{"command":"echo hi"}"#.to_string(),
        }]),
        // Naive cut (preserve_recent = 2) would land here — tail begins with
        // a bare tool_result whose matching tool_use is in the compacted prefix.
        ConversationMessage::tool_result("99", "bash", "hi", false),
        ConversationMessage::user_text("thanks"),
    ]);

    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
    };

    let plan = prepare_compaction(&session, config).expect("should compact");

    // The preserved tail must NOT begin with a tool_result block.
    let first_tail = plan
        .preserved_tail
        .first()
        .expect("preserved tail should be non-empty");
    assert!(
        !first_tail
            .blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. })),
        "preserved tail must not begin with an orphan tool_result"
    );

    // And the tail must contain the matching tool_use that pairs with the
    // pulled-in tool_result.
    let has_matching_tool_use = plan.preserved_tail.iter().any(|m| {
        m.blocks.iter().any(|b| {
            matches!(
                b,
                ContentBlock::ToolUse { id, .. } if id == "99"
            )
        })
    });
    assert!(
        has_matching_tool_use,
        "preserved tail must include the matching tool_use"
    );
}

#[test]
fn compact_summary_includes_key_file_paths_from_removed_messages() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text(
            "Please edit rust/crates/runtime/src/compact.rs and rust/crates/runtime/src/lib.rs",
        ),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Editing both files now.".to_string(),
        }]),
        ConversationMessage::user_text("also check rust/crates/core-types/src/session.rs"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "Done. remaining: write tests.".to_string(),
        }]),
        // tail (preserved)
        ConversationMessage::user_text("commit?"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "yes".to_string(),
        }]),
    ]);

    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
    };

    let result = compact_session(&session, config);

    // At least some of the referenced file paths must appear in the summary
    assert!(
        result.summary.contains("compact.rs")
            || result.summary.contains("lib.rs")
            || result.summary.contains("session.rs"),
        "summary should reference at least one key file path"
    );
}

// --- token estimation ---

#[test]
fn estimate_message_tokens_empty_text_returns_minimum_plus_overhead() {
    let msg = ConversationMessage::user_text("");
    // chars().count()/4 + 1 (block min) + 4 (message overhead) = 5
    assert_eq!(super::estimate_message_tokens(&msg), 5);
}

#[test]
fn estimate_message_tokens_scales_with_length() {
    let short = ConversationMessage::user_text("hi");
    let long = ConversationMessage::user_text("w ".repeat(400));
    let short_tokens = super::estimate_message_tokens(&short);
    let long_tokens = super::estimate_message_tokens(&long);
    assert!(
        long_tokens > short_tokens * 10,
        "longer text should produce substantially more tokens"
    );
}

#[test]
fn estimate_message_tokens_tool_use_counts_name_and_input() {
    let msg = ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id: "abc".to_string(),
        name: "bash".to_string(), // 4 chars
        input: "x".repeat(96),    // 96 chars → (4+96)/4+8 = 33
    }]);
    let tokens = super::estimate_message_tokens(&msg);
    // (4 + 96) / 4 + 8 (tool overhead) + 4 (message overhead) = 37
    assert_eq!(tokens, 37);
}

#[test]
fn estimate_session_tokens_sums_all_messages() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("aaaa"), // 4/4+1 + 4 = 6
        ConversationMessage::user_text("aaaa"), // 6
    ]);
    assert_eq!(estimate_session_tokens(&session), 12);
}

// --- UTF-8 / Unicode handling ---

#[test]
fn estimate_message_tokens_uses_byte_len_for_speed() {
    // "안녕" = 6 bytes in UTF-8, 2 chars
    // estimate_message_tokens uses len() (O(1)) not chars().count() (O(n)),
    // so 6/4+1 = 2 plus message overhead 4 = 6.
    // Slight overestimate for CJK is acceptable for compaction thresholds.
    let msg = ConversationMessage::user_text("안녕");
    let tokens = super::estimate_message_tokens(&msg);
    assert_eq!(tokens, 6);
}

#[test]
fn estimate_message_tokens_image_uses_fixed_estimate() {
    let msg = ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "x".repeat(100_000), // 100KB base64 — irrelevant
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    };
    let tokens = super::estimate_message_tokens(&msg);
    // 1600 (fixed image estimate) + 4 (message overhead) = 1604
    assert_eq!(tokens, 1604);
}

#[test]
fn compact_summary_mentions_images_without_copying_base64() {
    let base64_payload = "base64_payload_should_not_survive_".repeat(200);
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage {
            role: MessageRole::User,
            blocks: vec![
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: base64_payload.clone(),
                },
                ContentBlock::Text {
                    text: "please inspect the screenshot".to_string(),
                },
            ],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        },
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "I inspected it.".to_string(),
        }]),
        ConversationMessage::user_text("recent tail"),
    ]);

    let result = compact_session(
        &session,
        CompactionConfig {
            preserve_recent_messages: 1,
            max_estimated_tokens: 1,
        },
    );

    assert!(result.removed_message_count > 0);
    assert!(result.summary.contains("[image: image/png]"));
    assert!(!result.summary.contains("base64_payload_should_not_survive"));
    assert!(!result
        .formatted_summary
        .contains("base64_payload_should_not_survive"));
}

#[test]
fn truncate_summary_handles_multibyte_characters_safely() {
    // truncate_summary uses chars().count() so it should not split multibyte sequences
    let korean = "가나다라마바사아자차카타파하".repeat(20); // 280 chars
    let truncated = super::truncate_summary(&korean, 20);
    // must be exactly 21 chars: 20 kept + ellipsis
    assert_eq!(truncated.chars().count(), 21);
    assert!(truncated.ends_with('…'));
    // must be valid UTF-8 (no panic = success)
    let _ = truncated.as_str();
}

#[test]
fn summarize_messages_handles_emoji_in_text_blocks() {
    // summarize_messages (private) is exercised through compact_session
    let mut session = Session::new();
    let emoji_text = "🚀".repeat(100);
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text(emoji_text.clone()),
        ConversationMessage::assistant(vec![ContentBlock::Text { text: emoji_text }]),
        ConversationMessage::user_text("recent A"),
        ConversationMessage::user_text("recent B"),
        ConversationMessage::user_text("recent C"),
        ConversationMessage::user_text("recent D"),
    ]);
    // Should not panic and should produce a valid summary
    let result = compact_session(
        &session,
        CompactionConfig {
            preserve_recent_messages: 4,
            max_estimated_tokens: 1,
        },
    );
    assert!(result.removed_message_count > 0);
    assert!(!result.summary.is_empty());
}

// --- continuation message format ---

#[test]
fn continuation_message_contains_preamble() {
    let summary = "<summary>Some work done.</summary>";
    let msg = get_compact_continuation_message(summary, false, false, &[]);
    assert!(
        msg.starts_with("This session is being continued"),
        "continuation message must start with the preamble"
    );
}

#[test]
fn continuation_message_with_suppress_includes_resume_instruction() {
    let summary = "<summary>Some work done.</summary>";
    let msg = get_compact_continuation_message(summary, true, false, &[]);
    assert!(
        msg.contains("Resume directly"),
        "suppress_follow_up_questions=true must include direct-resume instruction"
    );
}

#[test]
fn continuation_message_without_suppress_omits_resume_instruction() {
    let summary = "<summary>Some work done.</summary>";
    let msg = get_compact_continuation_message(summary, false, false, &[]);
    assert!(
        !msg.contains("Resume directly"),
        "suppress_follow_up_questions=false must not include direct-resume instruction"
    );
}

#[test]
fn continuation_message_with_recent_preserved_includes_note() {
    let summary = "<summary>Some work done.</summary>";
    let msg = get_compact_continuation_message(summary, false, true, &[]);
    assert!(
        msg.contains("Recent messages are preserved verbatim"),
        "recent_messages_preserved=true must include the recent-messages note"
    );
}

#[test]
fn continuation_message_without_recent_preserved_omits_note() {
    let summary = "<summary>Some work done.</summary>";
    let msg = get_compact_continuation_message(summary, false, false, &[]);
    assert!(
        !msg.contains("Recent messages are preserved verbatim"),
        "recent_messages_preserved=false must not include the recent-messages note"
    );
}

// --- threshold validation ---

#[test]
fn default_compaction_config_has_reasonable_thresholds() {
    let config = CompactionConfig::default();
    // preserve_recent must be at least 2 so a round-trip exchange is kept
    assert!(config.preserve_recent_messages >= 2);
    // max token threshold should be in a sensible range (1K–1M)
    assert!(config.max_estimated_tokens >= 1_000);
    assert!(config.max_estimated_tokens <= 1_000_000);
}

#[test]
fn should_compact_threshold_is_not_crossed_by_default_sized_session() {
    // A typical short session (10 turns of ~20 words each) should not trigger compaction
    // with the default config.
    let mut session = Session::new();
    for _ in 0..10 {
        ::std::sync::Arc::make_mut(&mut session.messages)
            .push(ConversationMessage::user_text("short message here please"));
        ::std::sync::Arc::make_mut(&mut session.messages).push(ConversationMessage::assistant(
            vec![ContentBlock::Text {
                text: "OK, done.".to_string(),
            }],
        ));
    }
    assert!(!should_compact(&session, CompactionConfig::default()));
}

// --- CompactionSummarizer trait and prepare/apply split ---

#[test]
fn prepare_compaction_returns_none_for_small_session() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![ConversationMessage::user_text("hello")]);
    assert!(prepare_compaction(&session, CompactionConfig::default()).is_none());
}

#[test]
fn prepare_compaction_returns_plan_with_correct_boundaries() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("one ".repeat(200)),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "two ".repeat(200),
        }]),
        ConversationMessage::user_text("three ".repeat(200)),
        ConversationMessage::user_text("recent A"),
        ConversationMessage::user_text("recent B"),
    ]);
    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
    };

    let plan = prepare_compaction(&session, config).expect("should produce a plan");
    assert_eq!(plan.messages_to_compact.len(), 3);
    assert_eq!(plan.preserved_tail.len(), 2);
    assert!(plan.existing_anchor.is_none());
}

#[test]
fn apply_compaction_builds_result_from_custom_summary() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("one ".repeat(200)),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "two ".repeat(200),
        }]),
        ConversationMessage::user_text("recent"),
        ConversationMessage::user_text("tail"),
    ]);
    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
    };

    let plan = prepare_compaction(&session, config).expect("plan");
    let result = apply_compaction(
        plan,
        "<analysis>test</analysis>\n<summary>Custom LLM summary here.</summary>",
    );

    assert_eq!(result.removed_message_count, 2);
    assert!(result
        .formatted_summary
        .contains("Custom LLM summary here."));
    assert_eq!(
        result.compacted_session.messages[0].role,
        MessageRole::System
    );
}

#[test]
fn compact_session_with_custom_summarizer() {
    struct FixedSummarizer;
    impl CompactionSummarizer for FixedSummarizer {
        fn summarize(&self, _messages: &[ConversationMessage]) -> String {
            "<summary>Fixed test summary.</summary>".to_string()
        }
    }

    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("one ".repeat(200)),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "two ".repeat(200),
        }]),
        ConversationMessage::user_text("recent"),
        ConversationMessage::user_text("tail"),
    ]);
    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
    };

    let result = compact_session_with(&session, config, &FixedSummarizer);
    assert_eq!(result.removed_message_count, 2);
    assert!(result.formatted_summary.contains("Fixed test summary."));
}

#[test]
fn local_summarizer_matches_compact_session_output() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(vec![
        ConversationMessage::user_text("one ".repeat(200)),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "two ".repeat(200),
        }]),
        ConversationMessage::user_text("recent"),
        ConversationMessage::user_text("tail"),
    ]);
    let config = CompactionConfig {
        preserve_recent_messages: 2,
        max_estimated_tokens: 1,
    };

    let direct = compact_session(&session, config);
    let via_trait = compact_session_with(&session, config, &LocalSummarizer);

    assert_eq!(direct.summary, via_trait.summary);
    assert_eq!(direct.formatted_summary, via_trait.formatted_summary);
    assert_eq!(
        direct.removed_message_count,
        via_trait.removed_message_count
    );
}

#[test]
fn compaction_system_prompt_contains_required_tags() {
    assert!(super::COMPACTION_SYSTEM_PROMPT.contains("<analysis>"));
    assert!(super::COMPACTION_SYSTEM_PROMPT.contains("<summary>"));
}

/// The summary prompt follows Claude Code's structured 8-section template so a
/// resumed session keeps the request intent, touched files, errors, pending
/// tasks, and the exact current work — not just a terse 6-bullet digest.
#[test]
fn compaction_system_prompt_uses_claude_eight_section_template() {
    let prompt = super::COMPACTION_SYSTEM_PROMPT;
    for section in [
        "1. Primary Request and Intent:",
        "2. Key Technical Concepts:",
        "3. Files and Code Sections:",
        "4. Errors and Fixes:",
        "5. Problem Solving:",
        "6. All User Messages:",
        "7. Pending Tasks:",
        "8. Current Work:",
    ] {
        assert!(
            prompt.contains(section),
            "compaction prompt must contain section `{section}`"
        );
    }
    // The continuation hint that lets the assistant resume immediately.
    assert!(prompt.contains("Next Step:"));
}

/// 라이브 리포트 "압축이 정보를 다 버린다"의 처방: 보존 테일이 고정 4개가
/// 아니라 토큰 예산으로 산정된다 — 예산이 넉넉하면 최근 라운드 여러 개가
/// 통째로 남고, 플로어(4)와 캡(요약할 8개는 남기기)이 양끝을 지킨다.
#[test]
fn preserved_tail_len_follows_token_budget_with_floor_and_cap() {
    // 20 messages, each ~25 estimated tokens (100 chars / 4).
    let messages: Vec<ConversationMessage> = (0..20)
        .map(|i| {
            ConversationMessage::user_text(format!("message {i}: {}", "x".repeat(92)))
        })
        .collect();

    // Budget for roughly 10 messages' worth of tokens → strictly more than the
    // legacy 4, strictly less than the whole transcript.
    let ten_messages_budget = 10 * 30;
    let n = super::preserved_tail_len_for_budget(&messages, ten_messages_budget);
    assert!(n > 4, "budget must beat the legacy fixed tail: {n}");
    assert!(n <= 12, "budget must not swallow the transcript: {n}");

    // A tiny budget floors at the legacy default…
    assert_eq!(super::preserved_tail_len_for_budget(&messages, 0), 4);

    // …and a huge budget is capped so at least 8 messages remain to compact.
    assert_eq!(
        super::preserved_tail_len_for_budget(&messages, u64::MAX),
        12,
        "20 messages - MIN_COMPACTABLE_MESSAGES(8) = 12"
    );

    // A session too small to hold both bounds keeps the legacy floor —
    // small-session behavior stays byte-identical to the fixed-tail era.
    let small: Vec<ConversationMessage> = (0..6)
        .map(|i| ConversationMessage::user_text(format!("m{i}")))
        .collect();
    assert_eq!(super::preserved_tail_len_for_budget(&small, u64::MAX), 4);
}

/// P3: the summary-request copy elides oversized tool-result bodies and drops
/// image payloads, while everything under the cap passes through untouched —
/// and the SESSION copy is never mutated (the trim applies to the request
/// clone only).
#[test]
fn pretrim_for_summary_elides_oversized_results_and_images_only() {
    let huge = "x".repeat(20_000);
    let messages = vec![
        ConversationMessage::user_text("small message stays byte-identical"),
        tool_result_message(1, &huge, 0),
        tool_result_message(2, "small result", 0),
        ConversationMessage::user_with_images(
            "caption",
            vec![("image/png".to_string(), "payload".to_string())],
        ),
    ];

    let trimmed = super::pretrim_messages_for_summary(&messages);

    // Untouched messages are byte-identical clones.
    assert_eq!(trimmed[0], messages[0]);
    assert_eq!(trimmed[2], messages[2]);

    // The oversized body was middle-elided under the cap, keeping both ends.
    let Some(ContentBlock::ToolResult { output, .. }) = trimmed[1].blocks.first() else {
        panic!("trimmed message must keep its tool_result block");
    };
    assert!(output.chars().count() < huge.chars().count());
    assert!(output.starts_with('x') && output.ends_with('x'));

    // Image blocks become a text placeholder; the caption text survives.
    assert!(trimmed[3]
        .blocks
        .iter()
        .all(|block| !matches!(block, ContentBlock::Image { .. })));
    assert!(trimmed[3].blocks.iter().any(|block| matches!(
        block,
        ContentBlock::Text { text } if text == "[image omitted from summary input]"
    )));

    // Source messages were never mutated.
    let Some(ContentBlock::ToolResult { output, .. }) = messages[1].blocks.first() else {
        unreachable!()
    };
    assert_eq!(output.chars().count(), 20_000);
}

/// `plan_superseded_reminders` walks every `System` message, and a compaction
/// continuation is a `System` message whose summary prose can quote both the
/// reminder tag and the recall header — this audit produced such summaries.
/// Classifying the lineage by substring would sort that summary into the recall
/// family and, being older than the next real recall section, reclaim it: the
/// compressed history of the conversation, replaced by a placeholder.
#[test]
fn a_summary_quoting_the_recall_header_is_not_reclaimed_as_a_recall_section() {
    let mut session = Session::new();
    session.push_user_text("start").expect("message");

    let quoting_summary = format!(
        "<system-reminder>\n[zo:compaction-continuation]\nEarlier work covered the \
         `# Recalled memory` section and its `<system-reminder>` wrapper.\n{}\n</system-reminder>",
        "detail ".repeat(400)
    );
    session
        .push_message(reminder_message(&quoting_summary))
        .expect("continuation summary");
    session.push_user_text("work").expect("message");

    let real_recall = format!(
        "<system-reminder>\n# Recalled memory\nentry: {}\n</system-reminder>",
        "detail ".repeat(400)
    );
    session
        .push_message(reminder_message(&real_recall))
        .expect("recall section");
    session.push_user_text("work").expect("message");

    let cleared = microcompact_session(&mut session, 0, 1)
        .map_or(0, |event| event.cleared_superseded_reminders);
    assert_eq!(
        cleared, 0,
        "a summary that merely quotes the header is not a recall section"
    );
}


/// The cost of a firing is WHERE it rewrites, and the quote has to say so.
///
/// Both sessions clear the SAME six results. One has thirty messages of
/// history sitting behind the rewrite and one has them in front of it. Priced
/// by what they free — which is what the gate did — they are identical; priced
/// by what the provider must re-read, one costs the whole conversation and the
/// other costs almost nothing. The live ledger charged 17.1M tokens for that
/// difference, 33 of the rewrites landing at message index 2.
#[test]
fn the_quote_prices_a_head_rewrite_far_above_an_identical_tail_rewrite() {
    let clearable = "x".repeat(4_000);
    let filler = || ConversationMessage {
        role: MessageRole::Assistant,
        blocks: vec![ContentBlock::Text {
            text: "f".repeat(5_000),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    };

    // History BEHIND the rewrite: everything after message 0 is re-billed.
    let mut head = Session::new();
    let mut behind: Vec<_> = (0..11).map(|i| tool_result_message(i, &clearable, 0)).collect();
    behind.extend((0..30).map(|_| filler()));
    head.messages = ::std::sync::Arc::new(behind);

    // The same rewrite with the history IN FRONT of it: those 30 messages stay
    // cached, because nothing before the frontier is touched.
    let mut tail = Session::new();
    let mut ahead: Vec<_> = (0..30).map(|_| filler()).collect();
    ahead.extend((0..11).map(|i| tool_result_message(i, &clearable, 0)));
    tail.messages = ::std::sync::Arc::new(ahead);

    let head_quote = microcompact_quote(&head, 5, 240);
    let tail_quote = microcompact_quote(&tail, 5, 240);

    assert_eq!(
        head_quote.clearable_tokens, tail_quote.clearable_tokens,
        "the fixtures free the same amount — position is the only variable"
    );
    assert!(head_quote.clearable_tokens > 0, "precondition: something clears");
    assert_eq!(head_quote.frontier, Some(0));
    assert_eq!(tail_quote.frontier, Some(30));
    assert!(
        head_quote.rebilled_tokens > tail_quote.rebilled_tokens * 3,
        "a head rewrite must price far above a tail one: {} vs {}",
        head_quote.rebilled_tokens,
        tail_quote.rebilled_tokens
    );
    assert!(
        tail_quote.pays_back_within(5),
        "a cheap tail trim must be worth firing — declining it leaves the \
         pressure for full compaction, which costs an LLM round-trip"
    );
}

/// A pass that rewrites nothing invalidates nothing, so it is always worth
/// running — the payback arithmetic must not divide it out of existence.
#[test]
fn a_firing_that_rewrites_nothing_costs_nothing() {
    let quote = microcompact_quote(&Session::new(), 5, 240);
    assert_eq!(quote.frontier, None);
    assert_eq!(quote.rebilled_tokens, 0);
    assert!(quote.pays_back_within(1));
    assert!(quote.pays_back_within(0), "no cost means no bet to lose");
}

/// The estimate and the quote must never drift: the estimate is the quote's
/// own gain field, so a caller that gates on one and fires on the other is
/// guaranteed the batch it was quoted.
#[test]
fn the_clearable_estimate_is_the_quotes_own_number() {
    let mut session = Session::new();
    session.messages = ::std::sync::Arc::new(
        (0..12)
            .map(|i| tool_result_message(i, &"z".repeat(4_000), 0))
            .collect(),
    );
    assert_eq!(
        microcompact_clearable_estimate(&session, 5, 240),
        microcompact_quote(&session, 5, 240).clearable_tokens
    );
}
