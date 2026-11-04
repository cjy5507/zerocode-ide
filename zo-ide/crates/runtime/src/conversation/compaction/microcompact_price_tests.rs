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
    let between = f64::midpoint(with, without);
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
    let ceiling = runtime.hard_context_ceiling_tokens() + 1;
    let fired = runtime.maybe_microcompact_for_tokens(ceiling);
    assert!(fired.is_some(), "the last-resort trim runs at the hard ceiling");
}

/// The clear pays until the conversation reaches the full-compaction tier at
/// the pace it has been growing, counted from where the clear leaves it; a
/// fall between two answered requests (a trim, a compaction) is not growth.
/// With no pace on record the horizon is the token rule's bet. Past the tier
/// a summary that can run rewrites everything at the next gate, so the clear
/// has no requests to pay back in, whatever it takes off (t-81314).
#[test]
fn the_payback_horizon_is_the_requests_left_to_the_full_tier_at_the_recent_pace() {
    let mut runtime = runtime_on("claude-opus-5-5");
    let unpaced = runtime.microcompact_payback_horizon(400_000, 0);
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
    let horizon = runtime.microcompact_payback_horizon(full - 100_000, 0);
    assert!((horizon - 25.0).abs() < f64::EPSILON, "{horizon}");
    // A clear that takes 8k off first leaves 108k of room, not 100k.
    let cleared = runtime.microcompact_payback_horizon(full - 100_000, 8_000);
    assert!((cleared - 27.0).abs() < f64::EPSILON, "{cleared}");
    // Six messages: a summary can run, and it comes first.
    assert!(runtime.compaction_config_if_possible().is_some());
    assert!(runtime.microcompact_payback_horizon(full, 0).abs() < f64::EPSILON);
    assert!(runtime.microcompact_payback_horizon(full, 50_000).abs() < f64::EPSILON);
}

/// Past the full tier with no summary to run — a transcript too short to fold —
/// what rewrites everything next is the last-resort trim at the hard ceiling,
/// so the room runs to the ceiling, from where the clear leaves the
/// conversation (t-78937 P3-4, t-81314).
#[test]
fn past_the_full_tier_with_no_summary_the_payback_horizon_runs_to_the_ceiling() {
    let mut runtime = runtime_on("claude-opus-5-5");
    runtime.session.push_user_text("go").expect("task");
    for context in [100_000_u32, 104_000] {
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
    assert!(
        runtime.compaction_config_if_possible().is_none(),
        "three messages are too few for a summary to run on"
    );
    // The pace is 4k a request.
    let full = u64::from(runtime.auto_compaction_input_tokens_threshold());
    let ceiling = runtime.hard_context_ceiling_tokens();
    let past = runtime.microcompact_payback_horizon(full, 0);
    assert!((past - token_count(ceiling - full) / token_count(4_000)).abs() < f64::EPSILON, "{past}");
    let cleared = runtime.microcompact_payback_horizon(full, 20_000);
    assert!(
        (cleared - token_count(ceiling - full + 20_000) / token_count(4_000)).abs() < f64::EPSILON,
        "{cleared}"
    );
    assert!(runtime.microcompact_payback_horizon(ceiling, 0).abs() < f64::EPSILON);
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

/// One read: the answer that asked for it — thinking first when
/// `thinking_bytes` is not zero, and the provider's `usage` when given — and
/// its result.
fn read_round(
    runtime: &mut ConversationRuntime<NoopApiClient, StaticToolExecutor>,
    read: usize,
    thinking_bytes: usize,
    body_bytes: usize,
    usage: Option<TokenUsage>,
) {
    let id = format!("read-{read}");
    let mut blocks = Vec::new();
    if thinking_bytes > 0 {
        blocks.push(ContentBlock::Thinking {
            thinking: "t".repeat(thinking_bytes),
            signature: "s".to_string(),
        });
    }
    blocks.push(ContentBlock::ToolUse {
        id: id.clone(),
        name: "read_file".to_string(),
        input: format!("{{\"path\":\"src/file_{read}.rs\"}}"),
    });
    runtime
        .session
        .push_message(ConversationMessage::assistant_with_usage(blocks, usage))
        .expect("call");
    runtime
        .session
        .push_message(ConversationMessage::tool_result(id, "read_file", "x".repeat(body_bytes), false))
        .expect("result");
}

/// A request the size of the whole window: past the hard ceiling, where the
/// trim runs whatever its price.
fn trim_past_the_ceiling(runtime: &mut ConversationRuntime<NoopApiClient, StaticToolExecutor>) {
    let whole_window = runtime.context_window();
    assert!(runtime.maybe_microcompact_for_tokens(whole_window).is_some(), "the last-resort trim runs");
}

/// The red case for the saving (t-78937 P2-2): Opus 5.5 dropped the thinking
/// behind the first trim's frontier on the request after it, and drops it
/// unbilled from then on. A second trim whose frontier falls inside that
/// stretch orphans none of it again; what it gains is what it clears and the
/// thinking of the answers that came after the first trim — here too little
/// to pay for re-writing the rest within the gate's bet.
#[test]
fn a_second_trim_does_not_count_thinking_the_first_already_dropped_as_saved() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime_on("claude-opus-5-5");
    runtime
        .session
        .push_user_text("work through the repository")
        .expect("task");
    // The first trim clears the oldest six results; the answers from the
    // eighth on think at length, and all of them sit behind its frontier.
    let first = 6 + MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW;
    for read in 0..first {
        read_round(&mut runtime, read, if read > 6 { 40_000 } else { 0 }, 4_000, None);
    }
    trim_past_the_ceiling(&mut runtime);
    // Twenty answers after it, each thinking briefly.
    for read in first..first + 20 {
        read_round(&mut runtime, read, 400, 400, None);
    }

    let quote = crate::microcompact_quote(
        &runtime.session,
        MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW,
        MICROCOMPACT_MIN_OUTPUT_BYTES,
    );
    assert!(
        quote.orphaned_thinking_tokens > 10 * quote.clearable_tokens,
        "the second frontier falls inside the first trim's stretch: {quote:?}"
    );
    let threshold = runtime.microcompact_input_tokens_threshold();
    let fired = runtime.maybe_microcompact_for_tokens(threshold);
    assert!(
        fired.is_none(),
        "the second trim counted thinking the provider already dropped as saved: {quote:?}, fired {fired:?}"
    );
}

/// The red case for the cost (t-78937 P2-2): the thinking the first trim
/// orphaned is no longer billed, so re-writing the conversation after a
/// second trim does not write it either. Here the dropped thinking all sits
/// before the second frontier; without it the re-write is small and the
/// second trim pays for itself.
#[test]
fn a_second_trim_does_not_pay_to_rewrite_thinking_the_first_already_dropped() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime_on("claude-opus-5-5");
    runtime
        .session
        .push_user_text("work through the repository")
        .expect("task");
    // The first trim clears the oldest six results; the answers that asked
    // for results two to seven thought at length, behind its frontier and
    // before the second one.
    let first = 6 + MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW;
    for read in 0..first {
        read_round(&mut runtime, read, if (1..=6).contains(&read) { 40_000 } else { 0 }, 8_000, None);
    }
    trim_past_the_ceiling(&mut runtime);
    // Enough small results after it that the second trim clears every result
    // the first one kept.
    for read in first..first + MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW {
        read_round(&mut runtime, read, 0, 300, None);
    }

    let quote = crate::microcompact_quote(
        &runtime.session,
        MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW,
        MICROCOMPACT_MIN_OUTPUT_BYTES,
    );
    assert_eq!(quote.orphaned_thinking_tokens, 0, "no thinking after the second frontier: {quote:?}");
    let threshold = runtime.microcompact_input_tokens_threshold();
    let fired = runtime.maybe_microcompact_for_tokens(threshold);
    assert!(
        fired.is_some(),
        "the second trim priced re-writing thinking the provider no longer bills: {quote:?}"
    );
}

/// Thirty large results and then the 24 the trim keeps, one read to a round,
/// grown past the full-compaction tier at a pace of 10k a request: the last
/// answered request 70k over the tier. The context the next gate weighs, 80k
/// over it.
fn grown_past_the_full_tier(runtime: &mut ConversationRuntime<NoopApiClient, StaticToolExecutor>) -> u64 {
    runtime
        .session
        .push_user_text("work through the repository")
        .expect("task");
    let full = u64::from(runtime.auto_compaction_input_tokens_threshold());
    for read in 0..30 {
        read_round(runtime, read, 0, 40_000, None);
    }
    // The recent pace: 10k a request, the last answered request 70k over the
    // tier.
    let paced = MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW;
    for step in 0..paced {
        let usage = (step + 4 >= paced).then(|| {
            let over = u64::try_from(paced - step).expect("fits") * 10_000;
            let counted = u32::try_from(full + 80_000 - over).expect("fits");
            TokenUsage {
                input_tokens: 1,
                cache_read_input_tokens: counted - 1,
                ..TokenUsage::default()
            }
        });
        read_round(runtime, 30 + step, 0, 300, usage);
    }
    full + 80_000
}

/// The same trim in a transcript too short for a summary to run on: one answer
/// that called every tool at once, their results in one message, and the
/// answer behind them — four messages with the task, where a summary needs
/// more than the four it keeps. Thirty large results and the 24 the trim
/// keeps, as [`grown_past_the_full_tier`] has them, at the same pace and over
/// the tier by the same amount.
fn grown_in_one_batch_past_the_full_tier(
    runtime: &mut ConversationRuntime<NoopApiClient, StaticToolExecutor>,
) -> u64 {
    let counted = |tokens: u64| {
        let counted = u32::try_from(tokens).expect("fits");
        Some(TokenUsage {
            input_tokens: 1,
            cache_read_input_tokens: counted - 1,
            ..TokenUsage::default()
        })
    };
    runtime
        .session
        .push_user_text("work through the repository")
        .expect("task");
    let full = u64::from(runtime.auto_compaction_input_tokens_threshold());
    let reads = 30 + MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW;
    let calls = (0..reads)
        .map(|read| ContentBlock::ToolUse {
            id: format!("read-{read}"),
            name: "read_file".to_string(),
            input: format!("{{\"path\":\"src/file_{read}.rs\"}}"),
        })
        .collect();
    runtime
        .session
        .push_message(ConversationMessage::assistant_with_usage(calls, counted(full + 60_000)))
        .expect("calls");
    let mut results = ConversationMessage::tool_result("read-0", "read_file", "x".repeat(40_000), false);
    for read in 1..reads {
        let body_bytes = if read < 30 { 40_000 } else { 300 };
        results.blocks.push(ContentBlock::ToolResult {
            tool_use_id: format!("read-{read}"),
            tool_name: "read_file".to_string(),
            output: "x".repeat(body_bytes),
            is_error: false,
            images: Vec::new(),
        });
    }
    runtime.session.push_message(results).expect("results");
    runtime
        .session
        .push_message(ConversationMessage::assistant_with_usage(
            vec![ContentBlock::Text { text: "read them all".to_string() }],
            counted(full + 70_000),
        ))
        .expect("answer");
    full + 80_000
}

/// The red case past the full tier (t-78937 P3-4): when no summary can run and
/// the conversation stays over the full-compaction tier, the next thing that
/// re-writes everything is the last-resort trim at the hard ceiling. A trim
/// that pays for itself before the ceiling at the recent pace runs. Only where
/// no summary can run (t-81314 P3-2): where one can, it re-writes everything
/// first, and the next test holds the trim back for it.
#[test]
fn past_the_full_tier_a_trim_that_pays_back_before_the_ceiling_runs() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime_on("claude-opus-5-5");
    let context = grown_in_one_batch_past_the_full_tier(&mut runtime);
    let full = u64::from(runtime.auto_compaction_input_tokens_threshold());
    assert!(!runtime.request_reaches_hard_context_ceiling(context));
    assert!(
        runtime.compaction_config_if_possible().is_none(),
        "the transcript has too few messages for a summary to run on"
    );
    let quote = crate::microcompact_quote(
        &runtime.session,
        MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW,
        MICROCOMPACT_MIN_OUTPUT_BYTES,
    );
    let fired = runtime.maybe_microcompact_for_tokens(context);
    assert!(
        fired.is_some(),
        "{context} tokens, over the {full} tier and under the ceiling, with no summary to run: a trim that pays back before the ceiling was declined: {quote:?}"
    );
}

/// Past the full tier with a summary that can still run, the trim waits for
/// it as it did before the horizon reached for the ceiling: the summary
/// re-writes the conversation at the next gate, trim or no trim, and a trim
/// beforehand re-bills the prefix for nothing (t-81314 P3-2). A failed summary
/// call is no reason to run one — it folds locally; what keeps a conversation
/// over the tier is a transcript with nothing to summarize.
#[test]
fn past_the_full_tier_a_trim_waits_for_a_summary_that_can_still_run() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime_on("claude-opus-5-5");
    let context = grown_past_the_full_tier(&mut runtime);
    let full = u64::from(runtime.auto_compaction_input_tokens_threshold());
    assert!(!runtime.request_reaches_hard_context_ceiling(context));
    assert!(
        runtime.compaction_config_if_possible().is_some(),
        "the transcript has messages enough for a summary to run on"
    );
    let quote = crate::microcompact_quote(
        &runtime.session,
        MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW,
        MICROCOMPACT_MIN_OUTPUT_BYTES,
    );
    let fired = runtime.maybe_microcompact_for_tokens(context);
    assert!(
        fired.is_none(),
        "{context} tokens, over the {full} tier, with a summary due at the next gate: a trim ran ahead of it: {quote:?}"
    );
}

/// The pace the next test's conversation has been growing at, tokens a
/// request.
const PACE: u32 = 1_000;

/// `answers` answered requests, the last counted as the local estimate counts
/// the whole transcript and each one before it [`PACE`] fewer: a recent pace of
/// [`PACE`] a request, and a measured estimate ratio of one, so the provider's
/// tokens and the estimate's are the same number.
fn answered_at_the_pace(runtime: &mut ConversationRuntime<NoopApiClient, StaticToolExecutor>, answers: u32) {
    let estimate = u32::try_from(runtime.estimated_request_context_tokens()).expect("fits");
    for answer in 0..answers {
        let counted = estimate - PACE * (answers - 1 - answer);
        runtime
            .session
            .push_message(ConversationMessage::assistant_with_usage(
                vec![ContentBlock::Text { text: "step".to_string() }],
                Some(TokenUsage {
                    input_tokens: 1,
                    cache_read_input_tokens: counted - 1,
                    ..TokenUsage::default()
                }),
            ))
            .expect("answered request");
    }
}

/// The clear pays until the conversation reaches the full-compaction tier, and
/// it brings the conversation back from where it stands: the requests it has
/// are the room left AFTER the clear, not before it (t-81314 P3-3). Here the
/// clear takes 30k tokens off a conversation ten requests of room short of
/// the tier at a pace of 1k a request: ten requests of room before it, forty
/// after, and it needs more than the first and fewer than the second to pay
/// back.
#[test]
fn the_payback_horizon_counts_the_room_left_after_the_trim() {
    let _env = crate::test_env_lock();
    let model = "claude-opus-5-5";
    let mut runtime = runtime_on(model);
    tool_loop(&mut runtime, 30 + MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW, 4_000);
    answered_at_the_pace(&mut runtime, 4);

    let quote = crate::microcompact_quote(
        &runtime.session,
        MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW,
        MICROCOMPACT_MIN_OUTPUT_BYTES,
    );
    let rates = api::CacheRates::for_anthropic_model(model).expect("the price table prices Opus 5.5");
    let prices = CachePrices {
        read: rates.read,
        write: rates.write(crate::convert_messages::conversation_anchor_ttl()),
    };
    let breakeven = quote.breakeven_requests(prices, false).expect("a clear saves something");
    assert!(
        breakeven > 10.0 && breakeven < 39.0,
        "the clear needs {breakeven:.1} requests to pay back: more than the ten of room before it, fewer than the forty after it: {quote:?}"
    );

    let full = u64::from(runtime.auto_compaction_input_tokens_threshold());
    let context = full - 10 * u64::from(PACE);
    let fired = runtime.maybe_microcompact_for_tokens(context);
    assert!(
        fired.is_some(),
        "{context} tokens, ten requests of room short of the {full} tier before the clear and forty after it: a trim needing {breakeven:.1} requests was priced on the room before it: {quote:?}"
    );
}

/// What the gate keeps of each firing on a model that binds thinking to its
/// prefix (t-78937 P2-2): the stretch whose thinking the provider drops from
/// then on, merged with the stretches before it, cut back when the transcript
/// loses its end, and gone with the transcript. The next quote counts that
/// thinking neither as saved nor as re-written.
#[test]
fn each_firing_records_the_thinking_the_provider_drops_after_it() {
    let _env = crate::test_env_lock();
    let keep = MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW;
    let mut runtime = runtime_on("claude-opus-5-5");
    runtime
        .session
        .push_user_text("work through the repository")
        .expect("task");
    let first = 6 + keep;
    for read in 0..first {
        read_round(&mut runtime, read, 4_000, 4_000, None);
    }
    let frontier = crate::microcompact_quote(&runtime.session, keep, MICROCOMPACT_MIN_OUTPUT_BYTES)
        .frontier
        .expect("the trim rewrites the oldest results");
    trim_past_the_ceiling(&mut runtime);
    let length = runtime.session.messages.len();
    assert_eq!(runtime.dropped_thinking_spans, vec![frontier..length]);

    // Twenty answers after the trim, a hundred tokens of thinking each.
    for read in first..first + 20 {
        read_round(&mut runtime, read, 400, 400, None);
    }
    let quote = crate::compact::microcompact_quote_after_drops(
        &runtime.session,
        keep,
        MICROCOMPACT_MIN_OUTPUT_BYTES,
        &runtime.dropped_thinking_spans,
    );
    assert_eq!(quote.orphaned_thinking_tokens, 20 * 100, "{quote:?}");
    // Every answer but the first sits behind the first frontier, a thousand
    // tokens of thinking each.
    let unaware = crate::microcompact_quote(&runtime.session, keep, MICROCOMPACT_MIN_OUTPUT_BYTES);
    let dropped = u64::try_from(first - 1).expect("fits") * 1_000;
    assert_eq!(unaware.resent_tokens - quote.resent_tokens, dropped, "{quote:?} against {unaware:?}");

    trim_past_the_ceiling(&mut runtime);
    assert_eq!(runtime.dropped_thinking_spans, vec![frontier..runtime.session.messages.len()]);

    // A cancelled turn takes messages back from the end.
    std::sync::Arc::make_mut(&mut runtime.session.messages).truncate(length - 10);
    assert!(runtime.maybe_microcompact_for_tokens(0).is_none());
    assert_eq!(runtime.dropped_thinking_spans, vec![frontier..length - 10]);

    runtime.replace_session(Session::new());
    assert!(runtime.dropped_thinking_spans.is_empty());
}

/// A model that does not bind thinking to its prefix keeps its thinking
/// through a trim, so a firing on it records nothing.
#[test]
fn a_firing_on_a_model_that_keeps_its_thinking_records_nothing() {
    let _env = crate::test_env_lock();
    let mut runtime = runtime_on("gpt-5.5");
    runtime
        .session
        .push_user_text("work through the repository")
        .expect("task");
    for read in 0..6 + MICROCOMPACT_KEEP_RECENT_RESULTS_LARGE_WINDOW {
        read_round(&mut runtime, read, 4_000, 4_000, None);
    }
    trim_past_the_ceiling(&mut runtime);
    assert!(runtime.dropped_thinking_spans.is_empty());
}
