//! The local estimate brought to the provider's count where an absolute
//! budget meets it (t-78937 Z3).
//!
//! `chars / 4` and the provider disagree by a factor that depends on what the
//! transcript holds: Korean prose counts low, thinking the provider stopped
//! billing counts high. Over five sessions on this machine (2026-10-27/28) the
//! estimate ran at 0.75 to 1.67 times the provider's count; scaled by the
//! ratio measured on the previous answered request, 277 of 295 requests came
//! within 20% of what the provider billed. A budget written in tokens — the
//! trim's floor ([`MICROCOMPACT_MIN_CLEARED_TOKENS`]), the kept tail — means
//! the provider's tokens.

use super::super::{ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError, StaticToolExecutor};
use super::{auto_compaction_tail_budget, MICROCOMPACT_MIN_CLEARED_TOKENS};
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

/// Two old bodies worth three quarters of [`MICROCOMPACT_MIN_CLEARED_TOKENS`]
/// by the estimate, under the floor — one and a half times it by the count
/// of a provider that bills twice the estimate.
#[test]
fn the_trim_floor_is_counted_in_the_providers_tokens() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime();
    runtime.session.push_user_text("read these").expect("task");
    // Four bytes a token: each body is three eighths of the floor.
    let body_bytes = usize::try_from(MICROCOMPACT_MIN_CLEARED_TOKENS * 3 / 2).expect("fits");
    push_results(&mut runtime, 2, body_bytes);
    push_results(&mut runtime, 10, 300);
    answer_counted_at(&mut runtime, 2.0);

    let threshold = runtime.microcompact_input_tokens_threshold();
    let fired = runtime.maybe_microcompact_for_tokens(threshold);
    assert!(
        fired.is_some(),
        "three quarters of the floor by the estimate are one and a half times it by the provider's count"
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

/// The price rule weighs both sides in the estimate's own unit, so the ratio
/// cannot move it: the same loop on Opus 5.5 is declined whether the provider
/// bills the estimate or 2.1 times it, and the token rule takes a batch far
/// over the floor either way.
#[test]
fn the_ratio_moves_the_floor_and_never_the_price_decision() {
    let _env = crate::test_env_lock();
    for times in [1.0, 2.1] {
        let mut priced = runtime();
        priced.set_context_model("claude-opus-5-5");
        priced.session.push_user_text("read these").expect("task");
        push_results(&mut priced, 54, 4_000);
        answer_counted_at(&mut priced, times);
        let threshold = priced.microcompact_input_tokens_threshold();
        assert!(priced.maybe_microcompact_for_tokens(threshold).is_none(), "priced, ratio {times}");
        let measured = priced.context_estimate_ratio.expect("measured");
        assert!((measured - times).abs() < 0.001, "measured {measured} for {times}");

        let mut unpriced = runtime();
        unpriced.session.push_user_text("read these").expect("task");
        push_results(&mut unpriced, 20, 12_000);
        answer_counted_at(&mut unpriced, times);
        let threshold = unpriced.microcompact_input_tokens_threshold();
        assert!(unpriced.maybe_microcompact_for_tokens(threshold).is_some(), "unpriced, ratio {times}");
    }
}

/// What the ratio was measured against stops holding on a model switch and
/// after a full compaction; until the next fresh measurement it reads as one.
#[test]
fn a_model_switch_or_a_full_compaction_forgets_the_ratio() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime();
    runtime.session.push_user_text("read these").expect("task");
    push_results(&mut runtime, 30, 8_000);
    answer_counted_at(&mut runtime, 2.0);
    assert!(runtime.maybe_microcompact_for_tokens(0).is_none());
    assert!(runtime.context_estimate_ratio.is_some());
    runtime.set_context_model("claude-sonnet-5-5");
    assert_eq!(runtime.context_estimate_ratio, None);

    assert!(runtime.maybe_microcompact_for_tokens(0).is_none());
    assert!(runtime.context_estimate_ratio.is_some());
    let config = runtime.compaction_config_if_possible().expect("compactable");
    let result = crate::compact::compact_session(&runtime.session, config);
    assert!(runtime.finish_auto_compaction(result).is_some(), "a full compaction folded the transcript");
    assert_eq!(runtime.context_estimate_ratio, None);
    assert_eq!(
        runtime.provider_tokens_for_estimate(MICROCOMPACT_MIN_CLEARED_TOKENS),
        MICROCOMPACT_MIN_CLEARED_TOKENS
    );
}

/// A calibrated figure never leaves the range the gates already work in:
/// not below zero, not above the window the compaction tiers are a share of.
#[test]
fn a_calibrated_figure_stays_inside_the_compaction_window() {
    let mut runtime = runtime();
    let window = super::auto_compaction_window(runtime.context_window_for_guards());
    runtime.context_estimate_ratio = Some(1e9);
    assert_eq!(runtime.provider_tokens_for_estimate(4_000), window);
    assert_eq!(runtime.estimate_tokens_for_provider(40_000), 0);
    runtime.context_estimate_ratio = Some(1e-9);
    assert_eq!(runtime.provider_tokens_for_estimate(4_000), 0);
    assert_eq!(runtime.estimate_tokens_for_provider(40_000), window);
}
