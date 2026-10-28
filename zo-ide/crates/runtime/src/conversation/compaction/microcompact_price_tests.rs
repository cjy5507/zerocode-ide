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
    MicrocompactPricing, MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW,
    MICROCOMPACT_MIN_OUTPUT_BYTES, MICROCOMPACT_PAYBACK_REQUESTS,
};
use crate::compact::{token_count, CachePrices};
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::session::{ContentBlock, ConversationMessage, Session};
use crate::usage::TokenUsage;
use crate::MicrocompactQuote;

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

fn opus_prices(ttl: api::CacheTtl) -> CachePrices {
    let rates = api::CacheRates::for_anthropic_model("claude-opus-5-5").expect("the price table prices Opus 5.5");
    CachePrices {
        read: rates.read,
        write: rates.write(ttl),
    }
}

/// A firing that rewrites a 200k conversation to clear 40k, with 30k of
/// thinking behind its frontier.
fn sample_quote(frontier: Option<usize>) -> MicrocompactQuote {
    MicrocompactQuote {
        clearable_tokens: 40_000,
        rebilled_tokens: 90_000,
        frontier,
        resent_tokens: 200_000,
        orphaned_thinking_tokens: 30_000,
    }
}

#[test]
fn the_hour_long_write_takes_longer_to_pay_back_than_the_five_minute_one() {
    let five = sample_quote(Some(3))
        .breakeven_requests(opus_prices(api::CacheTtl::FiveMinutes), false)
        .expect("a clear saves something");
    let hour = sample_quote(Some(3))
        .breakeven_requests(opus_prices(api::CacheTtl::OneHour), false)
        .expect("a clear saves something");
    assert!(five > 0.0 && hour > five, "five minutes {five}, an hour {hour}");
}

#[test]
fn a_trim_that_rewrites_nothing_pays_back_at_once() {
    let free = sample_quote(None);
    assert!(free
        .breakeven_requests(opus_prices(api::CacheTtl::OneHour), false)
        .is_some_and(|requests| requests.abs() < f64::EPSILON));
    assert!(free.pays_back_at(opus_prices(api::CacheTtl::OneHour), 0.0, false));
}

#[test]
fn thinking_the_edit_orphans_counts_as_saved_only_where_the_model_drops_it() {
    let prices = opus_prices(api::CacheTtl::FiveMinutes);
    let with = sample_quote(Some(3)).breakeven_requests(prices, true).expect("saves");
    let without = sample_quote(Some(3)).breakeven_requests(prices, false).expect("saves");
    assert!(with < without, "with the dropped thinking {with}, without {without}");
    let between = (with + without) / 2.0;
    assert!(sample_quote(Some(3)).pays_back_at(prices, between, true));
    assert!(!sample_quote(Some(3)).pays_back_at(prices, between, false));
}

/// A model the price table does not price keeps the token comparison: the
/// same loop the red case declines on Opus 5.5 still fires.
#[test]
fn an_unpriced_model_keeps_the_token_rule() {
    let _env = crate::test_env_lock();
    let model = "gpt-5.5";
    assert!(api::CacheRates::for_anthropic_model(model).is_none());
    let mut runtime = runtime_on(model);
    tool_loop(&mut runtime, 30 + MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW, 4_000);
    let threshold = runtime.microcompact_input_tokens_threshold();
    let fired = runtime.maybe_microcompact_for_tokens(threshold);
    assert!(fired.is_some(), "the token rule takes this trim on an unpriced model");
}

/// The prices never stand between a request and the provider's ceiling: at
/// 95% of the real window the declined trim still runs.
#[test]
fn the_hard_ceiling_still_runs_a_trim_the_prices_decline() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime_on("claude-opus-5-5");
    tool_loop(&mut runtime, 30 + MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW, 4_000);
    assert!(runtime
        .maybe_microcompact_for_tokens(runtime.microcompact_input_tokens_threshold())
        .is_none());
    let ceiling = runtime.context_window() * 95 / 100 + 1;
    let fired = runtime.maybe_microcompact_for_tokens(ceiling);
    assert!(fired.is_some(), "the last-resort trim runs at the hard ceiling");
}

/// The clear pays until the conversation reaches the full-compaction tier at
/// the pace it has been growing; a fall between two answered requests (a
/// trim, a compaction) is not growth. With no pace on record the horizon is
/// the token rule's bet.
#[test]
fn the_payback_horizon_is_the_requests_left_to_the_full_tier_at_the_recent_pace() {
    let mut runtime = runtime_on("claude-opus-5-5");
    let unpaced = runtime.microcompact_payback_horizon(400_000);
    assert!((unpaced - token_count(MICROCOMPACT_PAYBACK_REQUESTS)).abs() < f64::EPSILON, "{unpaced}");
    runtime.session.push_user_text("go").expect("task");
    for context in [100_000_u32, 103_000, 107_000, 60_000, 64_000] {
        runtime
            .session
            .push_message(ConversationMessage::assistant_with_usage(
                vec![ContentBlock::Text { text: "step".to_string() }],
                Some(TokenUsage {
                    input_tokens: 1,
                    cache_read_input_tokens: context - 1,
                    ..TokenUsage::default()
                }),
            ))
            .expect("answered request");
    }
    // Rises 3k, 4k, 4k (the fall to 60k is skipped): the median is 4k.
    let full = u64::from(runtime.auto_compaction_input_tokens_threshold());
    let horizon = runtime.microcompact_payback_horizon(full - 100_000);
    assert!((horizon - 25.0).abs() < f64::EPSILON, "{horizon}");
    assert!(runtime.microcompact_payback_horizon(full).abs() < f64::EPSILON);
}

/// What a firing leaves on the break row: the quote in the gate's unit, the
/// table rates in thousandths of input, the lifetime, the horizon and the
/// break-even it needed.
#[test]
fn a_firing_leaves_its_quote_and_prices_for_the_ledger() {
    let pricing = MicrocompactPricing {
        prices: Some(opus_prices(api::CacheTtl::FiveMinutes)),
        ttl: api::CacheTtl::FiveMinutes,
        horizon_requests: 24.2,
        drops_orphaned_thinking: true,
    };
    let row = pricing.ledger_quote(&sample_quote(Some(3)), 416_000, 420_000, "pays_back");
    assert_eq!(row.clearable_tokens, 40_000);
    assert_eq!(row.resent_tokens, 200_000);
    assert_eq!(row.orphaned_thinking_tokens, 30_000);
    assert_eq!(row.frontier, Some(3));
    assert_eq!((row.threshold_tokens, row.context_tokens), (416_000, 420_000));
    assert_eq!((row.read_per_mille, row.write_per_mille), (Some(50), Some(1_250)));
    assert_eq!(row.ttl.as_deref(), Some("5m"));
    assert_eq!(row.horizon_requests, 25);
    // (200k - 70k) * (1.25 - 0.05) / (70k * 0.05) = 44.6 -> 45
    assert_eq!(row.breakeven_requests, Some(45));
    assert!(row.drops_orphaned_thinking);
    assert_eq!(row.decision, "pays_back");

    let unpriced = MicrocompactPricing { prices: None, ..pricing };
    let row = unpriced.ledger_quote(&sample_quote(Some(3)), 416_000, 420_000, "token_rule");
    assert_eq!((row.read_per_mille, row.write_per_mille, row.ttl), (None, None, None));
    assert_eq!(row.breakeven_requests, None);
}
