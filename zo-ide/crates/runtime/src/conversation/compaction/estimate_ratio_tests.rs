//! The local estimate brought to the provider's count where an absolute
//! budget meets it (t-78937 Z3).
//!
//! `chars / 4` and the provider disagree by a factor that depends on what the
//! transcript holds: Korean prose counts low, thinking the provider stopped
//! billing counts high. Over five sessions on this machine (2026-10-27/28) the
//! estimate ran at 0.75 to 1.67 times the provider's count; scaled by the
//! ratio measured on the previous answered request, 277 of 295 requests came
//! within 20% of what the provider billed. A budget written in tokens — the
//! trim's 4k floor, the kept tail — means the provider's tokens.

use super::super::{ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError, StaticToolExecutor};
use super::auto_compaction_tail_budget;
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::session::{ContentBlock, ConversationMessage, Session};
use crate::usage::TokenUsage;

struct NoopApiClient;

impl ApiClient for NoopApiClient {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        unreachable!("the gates never call the model")
    }
}

/// A runtime on a 200k window with no model named: no price applies, so the
/// trim decides by the token rule and only the floor stands in its way.
fn runtime() -> ConversationRuntime<NoopApiClient, StaticToolExecutor> {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        NoopApiClient,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::WorkspaceWrite),
        vec!["system".to_string()],
    );
    runtime.set_context_window(200_000);
    runtime
}

fn push_results(runtime: &mut ConversationRuntime<NoopApiClient, StaticToolExecutor>, count: usize, body_bytes: usize) {
    for result in 0..count {
        let id = format!("read-{}-{result}", runtime.session.messages.len());
        runtime
            .session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: id.clone(),
                name: "read_file".to_string(),
                input: "{}".to_string(),
            }]))
            .expect("call");
        runtime
            .session
            .push_message(ConversationMessage::tool_result(id, "read_file", "x".repeat(body_bytes), false))
            .expect("result");
    }
}

/// The answered request: the provider counted `times` the local estimate of
/// everything it was sent.
fn answer_counted_at(runtime: &mut ConversationRuntime<NoopApiClient, StaticToolExecutor>, times: f64) {
    let estimate = runtime.estimated_request_context_tokens();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let counted = (estimate as f64 * times).round() as u32;
    runtime
        .session
        .push_message(ConversationMessage::assistant_with_usage(
            vec![ContentBlock::Text { text: "next".to_string() }],
            Some(TokenUsage {
                input_tokens: 1,
                cache_read_input_tokens: counted - 1,
                ..TokenUsage::default()
            }),
        ))
        .expect("answer");
}

/// Two old bodies worth 3k estimated tokens, under the 4k floor — 6k by the
/// count of a provider that bills twice the estimate.
#[test]
fn the_trim_floor_is_counted_in_the_providers_tokens() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime();
    runtime.session.push_user_text("read these").expect("task");
    push_results(&mut runtime, 2, 6_000);
    push_results(&mut runtime, 10, 300);
    answer_counted_at(&mut runtime, 2.0);

    let threshold = runtime.microcompact_input_tokens_threshold();
    let fired = runtime.maybe_microcompact_for_tokens(threshold);
    assert!(
        fired.is_some(),
        "3k estimated tokens are 6k by the provider's count, over the 4k floor"
    );
}

/// The kept tail's budget is the provider's tokens: where the provider bills
/// twice the estimate, the tail holds what half the budget holds by the
/// estimate.
#[test]
fn the_kept_tail_holds_its_budget_in_the_providers_tokens() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime();
    runtime.session.push_user_text("read these").expect("task");
    push_results(&mut runtime, 30, 8_000);
    answer_counted_at(&mut runtime, 2.0);

    // The gate measures before it decides; below the tier it decides nothing.
    assert!(runtime.maybe_microcompact_for_tokens(0).is_none());
    let budget = auto_compaction_tail_budget(runtime.context_window_for_guards());
    let expected = crate::compact::preserved_tail_len_for_budget(&runtime.session.messages, budget / 2);
    let config = runtime.compaction_config_if_possible().expect("compactable");
    assert_eq!(
        config.preserve_recent_messages,
        expected,
        "a {budget}-token tail budget, with the provider billing twice the estimate"
    );
}
