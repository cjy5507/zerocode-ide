use super::{
    RateLimitSnapshot, RateLimitWindow, TokenUsage, UsageTracker, format_usd, pricing_for_model,
};

#[test]
fn rate_limit_window_used_percent_rounds_and_clamps() {
    assert_eq!(
        RateLimitWindow {
            utilization: 0.684,
            resets_at_unix: None,
        }
        .used_percent(),
        68
    );
    assert_eq!(
        RateLimitWindow {
            utilization: 1.5,
            resets_at_unix: None,
        }
        .used_percent(),
        100
    );
    assert_eq!(
        RateLimitWindow {
            utilization: -0.1,
            resets_at_unix: None,
        }
        .used_percent(),
        0
    );
}

#[test]
fn rate_limit_snapshot_has_data() {
    assert!(!RateLimitSnapshot::default().has_data());
    let snap = RateLimitSnapshot {
        five_hour: Some(RateLimitWindow {
            utilization: 0.1,
            resets_at_unix: None,
        }),
        ..Default::default()
    };
    assert!(snap.has_data());
}
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

#[test]
fn tracks_true_cumulative_usage() {
    let mut tracker = UsageTracker::new();
    tracker.record(TokenUsage {
        input_tokens: 10,
        output_tokens: 4,
        cache_creation_input_tokens: 2,
        cache_read_input_tokens: 1,
        output_tokens_details: None,
    });
    tracker.record(TokenUsage {
        input_tokens: 20,
        output_tokens: 6,
        cache_creation_input_tokens: 3,
        cache_read_input_tokens: 2,
        output_tokens_details: None,
    });

    assert_eq!(tracker.turns(), 2);
    assert_eq!(tracker.current_turn_usage().input_tokens, 20);
    assert_eq!(tracker.current_turn_usage().output_tokens, 6);
    assert_eq!(tracker.cumulative_usage().output_tokens, 10);
    assert_eq!(tracker.cumulative_usage().input_tokens, 30);
    assert_eq!(tracker.cumulative_usage().total_tokens(), 48);
}

#[test]
fn cumulative_usage_saturates_instead_of_overflowing() {
    // Regression: a long/imported session can push a `u32` cumulative counter
    // past its max. Plain `+=` panics in debug and wraps in release, corrupting
    // `/cost`. Recording near-max usage twice must clamp at `u32::MAX`, not wrap
    // or panic. Every accumulated field is exercised, including
    // `cache_read_input_tokens` (guards against it regressing to plain `+=`).
    let mut tracker = UsageTracker::new();
    let near_max = TokenUsage {
        input_tokens: u32::MAX - 1,
        output_tokens: u32::MAX - 1,
        cache_creation_input_tokens: u32::MAX - 1,
        cache_read_input_tokens: u32::MAX - 1,
        output_tokens_details: None,
    };
    tracker.record(near_max);
    tracker.record(near_max);

    let cumulative = tracker.cumulative_usage();
    assert_eq!(cumulative.input_tokens, u32::MAX);
    assert_eq!(cumulative.output_tokens, u32::MAX);
    assert_eq!(cumulative.cache_creation_input_tokens, u32::MAX);
    assert_eq!(cumulative.cache_read_input_tokens, u32::MAX);
    // new-input accumulator (input minus cache-read) also saturates; here
    // input == cache_read so each turn contributes 0, and the running total
    // stays 0 without underflowing.
    assert_eq!(tracker.cumulative_new_input_tokens(), 0);
    assert_eq!(tracker.turns(), 2);
}

fn assert_price(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < f64::EPSILON);
}

#[test]
fn deepseek_pricing_matches_v4_pro_and_flash_tiers() {
    let flash = pricing_for_model("deepseek-v4-flash").expect("deepseek flash pricing");
    assert_price(flash.input_cost_per_million, 0.27);
    assert_price(flash.cache_creation_cost_per_million, 0.27);
    assert_price(flash.cache_read_cost_per_million, 0.07);
    assert_price(flash.output_cost_per_million, 1.10);

    let pro = pricing_for_model("deepseek-v4-pro").expect("deepseek v4 pro pricing");
    assert_price(pro.input_cost_per_million, 0.55);
    assert_price(pro.cache_creation_cost_per_million, 0.55);
    assert_price(pro.cache_read_cost_per_million, 0.14);
    assert_price(pro.output_cost_per_million, 2.19);
}

#[test]
fn total_tokens_saturates_and_u64_preserves_large_totals() {
    let usage = TokenUsage {
        input_tokens: u32::MAX,
        output_tokens: 1,
        cache_creation_input_tokens: 2,
        cache_read_input_tokens: 3,
        output_tokens_details: None,
    };
    assert_eq!(usage.total_tokens(), u32::MAX);
    assert_eq!(usage.total_tokens_u64(), u64::from(u32::MAX) + 6);
}

#[test]
fn computes_cost_summary_lines() {
    let usage = TokenUsage {
        input_tokens: 1_000_000,
        output_tokens: 500_000,
        cache_creation_input_tokens: 100_000,
        cache_read_input_tokens: 200_000,
        output_tokens_details: None,
    };

    let cost = usage.estimate_cost_usd();
    assert_eq!(format_usd(cost.input_cost_usd), "$3.0000");
    assert_eq!(format_usd(cost.output_cost_usd), "$7.5000");
    let lines = usage.summary_lines_for_model("usage", Some("claude-sonnet-4-20250514"));
    assert!(lines[0].contains("estimated_cost=$10.9350"));
    assert!(lines[0].contains("model=claude-sonnet-4-20250514"));
    assert!(lines[1].contains("cache_read=$0.0600"));
}

#[test]
fn supports_model_specific_pricing() {
    let usage = TokenUsage {
        input_tokens: 1_000_000,
        output_tokens: 500_000,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        output_tokens_details: None,
    };

    let haiku = pricing_for_model("claude-haiku-4-5-20251001").expect("haiku pricing");
    let opus = pricing_for_model("claude-opus-4-6").expect("opus pricing");
    let haiku_cost = usage.estimate_cost_usd_with_pricing(haiku);
    let opus_cost = usage.estimate_cost_usd_with_pricing(opus);
    assert_eq!(format_usd(haiku_cost.total_cost_usd()), "$3.5000");
    // Opus 4.x is $5/$25: 1M input + 0.5M output = $5 + $12.5.
    assert_eq!(format_usd(opus_cost.total_cost_usd()), "$17.5000");
}

#[test]
fn marks_unknown_model_pricing_as_fallback() {
    let usage = TokenUsage {
        input_tokens: 100,
        output_tokens: 100,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        output_tokens_details: None,
    };
    let lines = usage.summary_lines_for_model("usage", Some("custom-model"));
    assert!(lines[0].contains("pricing=estimated-default"));
}

// ---- pricing_for_model alias coverage (Task 1) -------------------------

#[test]
fn pricing_for_model_sonnet_aliases() {
    assert!(
        pricing_for_model("claude-3-5-sonnet-20241022").is_some(),
        "full dated sonnet alias"
    );
    assert!(
        pricing_for_model("claude-sonnet-4-20250514").is_some(),
        "short dated sonnet alias"
    );
    let p = pricing_for_model("claude-3-5-sonnet-20241022").unwrap();
    assert!((p.input_cost_per_million - 3.0).abs() < f64::EPSILON);
}

#[test]
fn pricing_for_model_opus_aliases() {
    assert!(
        pricing_for_model("claude-3-opus-20240229").is_some(),
        "full dated opus alias"
    );
    assert!(
        pricing_for_model("claude-opus-4-6").is_some(),
        "short opus alias"
    );
    let p = pricing_for_model("claude-opus-4-6").unwrap();
    assert!((p.input_cost_per_million - 5.0).abs() < f64::EPSILON);
    assert!((p.output_cost_per_million - 25.0).abs() < f64::EPSILON);
}

#[test]
fn pricing_for_model_haiku_alias() {
    assert!(
        pricing_for_model("claude-haiku-4-5-20251001").is_some(),
        "full dated haiku alias"
    );
    let p = pricing_for_model("claude-haiku-4-5-20251001").unwrap();
    assert!((p.input_cost_per_million - 1.0).abs() < f64::EPSILON);
    assert!((p.output_cost_per_million - 5.0).abs() < f64::EPSILON);
}

#[test]
fn pricing_for_model_unknown_returns_none() {
    assert!(
        pricing_for_model("mistral-large-2").is_none(),
        "unknown model family"
    );
    assert!(pricing_for_model("").is_none(), "empty string");
}

#[test]
fn pricing_is_per_provider_and_distinct() {
    // The whole point: each family prices at its own rate, and every provider
    // has a cache-read discount (not just Claude).
    let opus = pricing_for_model("claude-opus-4-8").unwrap();
    let sonnet = pricing_for_model("claude-sonnet-4-6").unwrap();
    let gpt = pricing_for_model("gpt-5.5").unwrap();
    let flash = pricing_for_model("gemini-3.5-flash").unwrap();
    let pro = pricing_for_model("gemini-3.1-pro-preview").unwrap();
    let grok = pricing_for_model("grok-3").unwrap();

    // Opus and Sonnet must differ (the original bug folded them together).
    assert!((opus.input_cost_per_million - 5.0).abs() < f64::EPSILON);
    assert!((sonnet.input_cost_per_million - 3.0).abs() < f64::EPSILON);
    assert!(
        (opus.output_cost_per_million - sonnet.output_cost_per_million).abs() > f64::EPSILON,
        "opus and sonnet output rates must differ"
    );

    // Gemini Flash is much cheaper than Opus — distinct, not the shared default.
    assert!((flash.input_cost_per_million - 1.5).abs() < f64::EPSILON);
    assert!((pro.input_cost_per_million - 2.0).abs() < f64::EPSILON);
    assert!((gpt.output_cost_per_million - 30.0).abs() < f64::EPSILON);
    assert!((grok.input_cost_per_million - 3.0).abs() < f64::EPSILON);

    // Every provider carries a cache-read rate (~0.1× input), none zero.
    for p in [opus, sonnet, gpt, flash, pro, grok] {
        assert!(
            p.cache_read_cost_per_million > 0.0,
            "cache read must be set"
        );
        assert!(
            p.cache_read_cost_per_million < p.input_cost_per_million,
            "cache read is a discount on input"
        );
    }
}

#[test]
fn pricing_for_model_gpt56_family_is_pinned_to_standard_tier() {
    for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        let pricing = pricing_for_model(model).expect("gpt-5.6 pricing");
        assert!((pricing.input_cost_per_million - 2.5).abs() < f64::EPSILON, "{model}");
        assert!((pricing.output_cost_per_million - 15.0).abs() < f64::EPSILON, "{model}");
        assert!((pricing.cache_creation_cost_per_million - 2.5).abs() < f64::EPSILON, "{model}");
        assert!((pricing.cache_read_cost_per_million - 0.25).abs() < f64::EPSILON, "{model}");
    }

    let premium = pricing_for_model("gpt-5.5").expect("gpt-5.5 pricing");
    assert!((premium.output_cost_per_million - 30.0).abs() < f64::EPSILON);
}

#[test]
fn pricing_for_model_case_insensitive() {
    assert!(pricing_for_model("Claude-Sonnet-4").is_some());
    assert!(pricing_for_model("CLAUDE-OPUS-4").is_some());
    assert!(pricing_for_model("Claude-HAIKU-3").is_some());
}

// ---- cumulative_new_input_tokens (Task 2) --------------------------------

#[test]
fn tracks_new_input_tokens_excluding_cache_reads() {
    let mut tracker = UsageTracker::new();
    // Turn 1: 100 input, 10 from cache — new = 90
    tracker.record(TokenUsage {
        input_tokens: 100,
        output_tokens: 5,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 10,
        output_tokens_details: None,
    });
    // Turn 2: 200 input, 50 from cache — new = 150
    tracker.record(TokenUsage {
        input_tokens: 200,
        output_tokens: 8,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 50,
        output_tokens_details: None,
    });

    assert_eq!(tracker.cumulative_new_input_tokens(), 240);
}

#[test]
fn new_input_tokens_saturates_when_cache_exceeds_input() {
    let mut tracker = UsageTracker::new();
    // Pathological: cache_read > input_tokens — result must not underflow.
    tracker.record(TokenUsage {
        input_tokens: 5,
        output_tokens: 1,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 100,
        output_tokens_details: None,
    });
    assert_eq!(tracker.cumulative_new_input_tokens(), 0);
}

#[test]
fn new_input_summary_line_format() {
    let mut tracker = UsageTracker::new();
    tracker.record(TokenUsage {
        input_tokens: 50,
        output_tokens: 10,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        output_tokens_details: None,
    });
    let line = tracker.new_input_summary_line();
    assert!(
        line.contains("new_input_tokens_cumulative=50"),
        "unexpected line: {line}"
    );
}

#[test]
fn reconstructs_usage_from_session_messages() {
    let mut session = Session::new();
    session.messages = std::sync::Arc::new(vec![ConversationMessage {
        role: MessageRole::Assistant,
        blocks: vec![ContentBlock::Text {
            text: "done".to_string(),
        }],
        usage: Some(TokenUsage {
            input_tokens: 5,
            output_tokens: 2,
            cache_creation_input_tokens: 1,
            cache_read_input_tokens: 0,
            output_tokens_details: None,
        }),
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    }]);

    let tracker = UsageTracker::from_session(&session);
    assert_eq!(tracker.turns(), 1);
    assert_eq!(tracker.cumulative_usage().total_tokens(), 8);
}

/// Fable 5.1 reads its cache at a quarter of Fable 5's rate — the one number the
/// two releases disagree on, so the point release must not fold into its parent.
#[test]
fn fable_5_1_reads_its_cache_at_a_quarter_of_fable_5() {
    let fable_5_1 = pricing_for_model("claude-fable-5-1").expect("fable 5.1 pricing");
    let fable_5 = pricing_for_model("claude-fable-5").expect("fable 5 pricing");
    let same = |left: f64, right: f64| (left - right).abs() < f64::EPSILON;
    assert!(same(fable_5_1.input_cost_per_million, fable_5.input_cost_per_million));
    assert!(same(fable_5_1.output_cost_per_million, fable_5.output_cost_per_million));
    assert!(same(fable_5_1.cache_read_cost_per_million, 0.25));
    assert!(same(fable_5.cache_read_cost_per_million, 1.0));
}
