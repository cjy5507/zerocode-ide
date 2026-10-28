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
