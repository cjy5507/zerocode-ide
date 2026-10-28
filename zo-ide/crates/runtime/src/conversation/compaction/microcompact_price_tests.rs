//! The tier-1 trim priced at the session model's own cache rates (t-78937 Z1).
//!
//! A firing writes the conversation back into the prompt cache once, at the
//! write rate, and saves only the read rate on every request after it. The
//! token-only rule (`clearable × 5 ≥ rebilled`) treats both sides as the same
//! price, so it takes trims that cost more than they can ever return: over 36
//! hours on this machine (2026-10-27/28, Opus 5.5) 69 firings re-wrote 13.35M
//! tokens to clear 2.79M.

use super::super::{ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError, StaticToolExecutor};
use super::{
    MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW, MICROCOMPACT_MIN_OUTPUT_BYTES,
    MICROCOMPACT_PAYBACK_REQUESTS,
};
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::session::{ContentBlock, ConversationMessage, Session};

struct NoopApiClient;

impl ApiClient for NoopApiClient {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        unreachable!("the trim gate never calls the model")
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

/// One task, then `reads` read calls each answered with a `body_bytes` body —
/// the shape of a long tool loop. The trim clears every result but the most
/// recent [`MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW`].
fn tool_loop(
    runtime: &mut ConversationRuntime<NoopApiClient, StaticToolExecutor>,
    reads: usize,
    body_bytes: usize,
) {
    runtime
        .session
        .push_user_text("work through the repository")
        .expect("task");
    for read in 0..reads {
        let id = format!("read-{read}");
        runtime
            .session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: id.clone(),
                name: "read_file".to_string(),
                input: format!("{{\"path\":\"src/file_{read}.rs\"}}"),
            }]))
            .expect("call");
        runtime
            .session
            .push_message(ConversationMessage::tool_result(
                id,
                "read_file",
                "x".repeat(body_bytes),
                false,
            ))
            .expect("result");
    }
}

/// The red case: a quote the token-only rule takes, in the gate's own unit
/// (both sides `chars / 4`), that the model's cache prices say can never pay
/// back within the horizon the gate bets on.
#[test]
fn a_trim_that_cannot_pay_back_at_the_models_cache_prices_is_declined() {
    let _env = crate::test_env_lock();
    let model = "claude-opus-5-5";
    let mut runtime = runtime_on(model);
    tool_loop(&mut runtime, 30 + MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW, 4_000);

    let quote = crate::microcompact_quote(
        &runtime.session,
        MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW,
        MICROCOMPACT_MIN_OUTPUT_BYTES,
    );
    assert!(
        quote.pays_back_within(MICROCOMPACT_PAYBACK_REQUESTS),
        "the token-only rule takes this firing: {quote:?}"
    );

    // Priced leniently — the cheaper five-minute write, and only the suffix
    // from the frontier re-written — it still needs far more requests than
    // the gate bets on.
    let rates = api::CacheRates::for_anthropic_model(model).expect("the price table prices Opus 5.5");
    #[allow(clippy::cast_precision_loss)]
    let breakeven = quote.rebilled_tokens as f64 * rates.write(api::CacheTtl::FiveMinutes)
        / (quote.clearable_tokens as f64 * rates.read);
    #[allow(clippy::cast_precision_loss)]
    let horizon = MICROCOMPACT_PAYBACK_REQUESTS as f64;
    assert!(breakeven > horizon, "break-even {breakeven:.1} requests: {quote:?}");

    let threshold = runtime.microcompact_input_tokens_threshold();
    let fired = runtime.maybe_microcompact_for_tokens(threshold);
    assert!(
        fired.is_none(),
        "a trim needing {breakeven:.0} requests to pay back ran within a {horizon} request bet: {fired:?}"
    );
}
