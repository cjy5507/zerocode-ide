//! The context the compaction gates compare against their tiers, in the
//! provider's own count (t-78937 Z2).
//!
//! Opus 5.5 binds each thinking block to the prefix it was produced under,
//! and zo asks the provider to drop a block whose prefix changed
//! (`thinking.block_binding: drop_block`). A microcompact changes the prefix,
//! so from then on the provider stops billing every thinking block after the
//! trim — while the local estimate keeps counting each one, text and signature
//! alike. Taking the larger of the two put the gates on the estimate: over 36
//! hours on this machine (2026-10-27/28) 51 of 69 trims fired below the
//! microcompact tier by the provider's count (p50 303k against 416k), and
//! full compaction ran at 230k–320k against its 520k tier.

use super::super::{ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError, StaticToolExecutor};
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::session::{ContentBlock, ConversationMessage, Session};
use crate::usage::TokenUsage;

struct NoopApiClient;

impl ApiClient for NoopApiClient {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        unreachable!("the gates never call the model")
    }
}

fn runtime_on(model: &str) -> ConversationRuntime<NoopApiClient, StaticToolExecutor> {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        NoopApiClient,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::WorkspaceWrite),
        vec!["system".to_string()],
    );
    runtime.set_context_model(model);
    runtime
}

/// One long turn on Opus 5.5 whose `steps` earlier answers each carry a
/// thinking block the provider no longer bills, then the last answered
/// request — the provider counted `provider_context` — and one small result.
fn turn_after_a_trim(
    runtime: &mut ConversationRuntime<NoopApiClient, StaticToolExecutor>,
    steps: usize,
    provider_context: u32,
) -> TokenUsage {
    runtime.session.push_user_text("keep going").expect("task");
    for step in 0..steps {
        let id = format!("call-{step}");
        runtime
            .session
            .push_message(ConversationMessage::assistant(vec![
                ContentBlock::Thinking {
                    thinking: "t".repeat(4_000),
                    signature: "s".repeat(48_000),
                },
                ContentBlock::ToolUse {
                    id: id.clone(),
                    name: "read_file".to_string(),
                    input: format!("{{\"path\":\"src/{step}.rs\"}}"),
                },
            ]))
            .expect("answer");
        runtime
            .session
            .push_message(ConversationMessage::tool_result(id, "read_file", "ok", false))
            .expect("result");
    }
    let usage = TokenUsage {
        input_tokens: 1,
        cache_read_input_tokens: provider_context - 1,
        output_tokens: 500,
        ..TokenUsage::default()
    };
    runtime
        .session
        .push_message(ConversationMessage::assistant_with_usage(
            vec![ContentBlock::ToolUse {
                id: "last".to_string(),
                name: "read_file".to_string(),
                input: "{\"path\":\"src/last.rs\"}".to_string(),
            }],
            Some(usage),
        ))
        .expect("last answer");
    runtime.usage_tracker.record(usage);
    runtime
        .session
        .push_message(ConversationMessage::tool_result("last", "read_file", "x".repeat(2_000), false))
        .expect("last result");
    usage
}

/// The red case: the provider counted 300k, the local estimate of the same
/// transcript is past the full-compaction tier because of thinking the
/// provider dropped. The gates must stand on the provider's count.
#[test]
fn the_gates_stand_on_the_providers_count_not_on_thinking_it_dropped() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime_on("claude-opus-5-5");
    turn_after_a_trim(&mut runtime, 45, 300_000);

    let micro = runtime.microcompact_input_tokens_threshold();
    let full = u64::from(runtime.auto_compaction_input_tokens_threshold());
    let estimate = runtime.estimated_request_context_tokens();
    assert!(estimate > full, "the local estimate counts the dropped thinking: {estimate}");

    let gated = runtime.effective_context_tokens();
    assert!(
        gated < micro,
        "the provider counted 300k but the gate compared {gated} (local estimate {estimate}) against the {micro} tier"
    );
    assert!(
        runtime.auto_compaction_config_if_preflight_ready().is_none(),
        "a turn's first request compacts on the local estimate {estimate} though the provider counted 300k"
    );
}

/// The figure is the provider's count of the last answered request and the
/// local estimate of what came after the answer.
#[test]
fn the_anchored_figure_adds_what_came_after_the_answer() {
    let mut runtime = runtime_on("claude-opus-5-5");
    let usage = turn_after_a_trim(&mut runtime, 3, 300_000);
    let after = runtime.session.messages.len() - 1;
    let since = crate::compact::estimate_messages_tokens(&runtime.session.messages[after..]);
    let expected = u64::from(usage.context_tokens()) + since;
    assert_eq!(runtime.provider_anchored_context_tokens(), Some(expected));
    assert_eq!(runtime.effective_context_tokens(), expected);
    assert_eq!(runtime.preflight_context_tokens(), expected);
}

/// Before any answer the local estimate stands alone, as it always did.
#[test]
fn without_an_answer_the_local_estimate_stands_alone() {
    let mut runtime = runtime_on("claude-opus-5-5");
    runtime.session.push_user_text("first prompt").expect("prompt");
    assert_eq!(runtime.provider_anchored_context_tokens(), None);
    assert_eq!(runtime.preflight_context_tokens(), runtime.estimated_request_context_tokens());
    assert_eq!(runtime.effective_context_tokens(), runtime.estimated_request_context_tokens());
}

/// An answer the last compaction kept in its tail was counted against the
/// history the compaction folded away, so it anchors nothing; the first
/// answer after the compaction does.
#[test]
fn an_answer_the_compaction_kept_in_its_tail_is_not_an_anchor() {
    let mut runtime = runtime_on("claude-opus-5-5");
    turn_after_a_trim(&mut runtime, 2, 300_000);
    runtime.session.compaction = Some(crate::SessionCompaction {
        count: 1,
        removed_message_count: 10,
        summary: "folded".to_string(),
        first_kept_message_index: None,
        anchor: None,
    });
    let messages = std::sync::Arc::make_mut(&mut runtime.session.messages);
    let continuation_at = messages
        .iter()
        .filter_map(|message| message.updated_at_ms)
        .max()
        .expect("pushed messages carry a time")
        + 1;
    messages[0].updated_at_ms = Some(continuation_at);
    assert_eq!(runtime.provider_anchored_context_tokens(), None);
    assert_eq!(
        runtime.effective_context_tokens(),
        u64::from(runtime.usage_tracker.current_turn_usage().context_tokens())
            .max(runtime.estimated_request_context_tokens())
    );

    let fresh = TokenUsage {
        input_tokens: 1,
        cache_read_input_tokens: 79_999,
        output_tokens: 200,
        ..TokenUsage::default()
    };
    let mut answer =
        ConversationMessage::assistant_with_usage(vec![ContentBlock::Text { text: "done".to_string() }], Some(fresh));
    answer.updated_at_ms = Some(continuation_at + 1);
    std::sync::Arc::make_mut(&mut runtime.session.messages).push(answer);
    assert_eq!(runtime.provider_anchored_context_tokens(), Some(80_000));
}

/// A firing's quote says what the gate's figure and tiers came from: the
/// provider's count and the local estimate apart, the window and the 650k
/// cap the tiers are a share of, the two overrides, the share, the model.
#[test]
fn a_firing_stamps_where_its_figure_and_tiers_came_from() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime_on("claude-opus-5-5");
    turn_after_a_trim(&mut runtime, 45, 300_000);
    let mut row = api::ContextTrimQuote::default();
    runtime.stamp_context_sources(&mut row);
    assert_eq!(row.provider_tokens, Some(300_000));
    assert_eq!(row.estimate_tokens, runtime.estimated_request_context_tokens());
    assert_eq!(row.window_tokens, runtime.context_window());
    assert_eq!(row.compaction_window_tokens, super::auto_compaction_window(runtime.context_window()));
    assert_eq!(
        (row.compaction_window_env, row.threshold_env),
        (
            std::env::var_os(super::AUTO_COMPACT_WINDOW_ENV_VAR).is_some(),
            std::env::var_os(super::AUTO_COMPACTION_THRESHOLD_ENV_VAR).is_some()
        )
    );
    assert_eq!(
        row.microcompact_percent * row.compaction_window_tokens / 100,
        runtime.microcompact_input_tokens_threshold()
    );
    assert_eq!(row.model, "claude-opus-5-5");
    assert_eq!(row.microcompact_streak, 0);
}
