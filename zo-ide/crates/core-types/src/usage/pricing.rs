//! Per-million-token pricing data, model→pricing lookup, the derived cost
//! estimate type, and dollar formatting for the usage ledger.

// Generic fallback for an unrecognized model: the Claude Sonnet 4.6 tier
// ($3 / $15), a mid-range rate. Used by `estimate_cost_usd()` (no model known)
// and as the `unwrap_or_else` fallback at the cost call sites.
const DEFAULT_INPUT_COST_PER_MILLION: f64 = 3.0;
const DEFAULT_OUTPUT_COST_PER_MILLION: f64 = 15.0;
const DEFAULT_CACHE_CREATION_COST_PER_MILLION: f64 = 3.75;
const DEFAULT_CACHE_READ_COST_PER_MILLION: f64 = 0.30;

/// Per-million-token pricing used for cost estimation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub cache_creation_cost_per_million: f64,
    pub cache_read_cost_per_million: f64,
}

impl ModelPricing {
    #[must_use]
    pub const fn default_sonnet_tier() -> Self {
        Self {
            input_cost_per_million: DEFAULT_INPUT_COST_PER_MILLION,
            output_cost_per_million: DEFAULT_OUTPUT_COST_PER_MILLION,
            cache_creation_cost_per_million: DEFAULT_CACHE_CREATION_COST_PER_MILLION,
            cache_read_cost_per_million: DEFAULT_CACHE_READ_COST_PER_MILLION,
        }
    }
}

/// Estimated dollar cost derived from a [`TokenUsage`](super::TokenUsage) sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageCostEstimate {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub cache_creation_cost_usd: f64,
    pub cache_read_cost_usd: f64,
}

impl UsageCostEstimate {
    #[must_use]
    pub fn total_cost_usd(self) -> f64 {
        self.input_cost_usd
            + self.output_cost_usd
            + self.cache_creation_cost_usd
            + self.cache_read_cost_usd
    }
}

/// Per-million-token rates `(input, output, cache_write, cache_read)`.
const fn tier(input: f64, output: f64, cache_write: f64, cache_read: f64) -> ModelPricing {
    ModelPricing {
        input_cost_per_million: input,
        output_cost_per_million: output,
        cache_creation_cost_per_million: cache_write,
        cache_read_cost_per_million: cache_read,
    }
}

/// Returns per-million-token pricing for a known model family, so each model's
/// cost — including its prompt-cache read discount — is estimated at its own
/// rate rather than a single shared default.
///
/// Matched by family substring (case-insensitive). Rates are public list prices
/// per 1M tokens; every provider has a cache-read discount (≈0.1× input). Only
/// Anthropic charges a cache-*write* premium (1.25× input); for the others the
/// cache-write rate equals input (no separate write charge). Zo runs these
/// over OAuth/subscription auth, so the dollar figure is a notional estimate,
/// not the metered bill.
///
/// Sources: Claude — platform.claude.com (authoritative, 2026-06); `DeepSeek` —
/// official API pricing-details-usd page exposes cache-hit/cache-miss/output
/// rows. Zo maps `DeepSeek` V4 Pro to the premium/pro row ($0.14 cache-hit /
/// $0.55 cache-miss input / $2.19 output) and `DeepSeek` Flash to the flash/chat
/// row ($0.07 / $0.27 / $1.10); GPT / Gemini / Grok — provider public pricing
/// pages (2026-06). GPT-5.6 Sol/Terra/Luna and Gemini/Grok rates are best-effort
/// public list prices; update when they move.
#[must_use]
pub fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    let m = model.to_ascii_lowercase();
    // --- Anthropic / Claude (authoritative) ---
    if m.contains("haiku") {
        return Some(tier(1.0, 5.0, 1.25, 0.10));
    }
    // Claude Fable 5.1 shares Fable 5's rates except cache reads, which fell to
    // $0.25 per MTok (0.025x input), a quarter of Fable 5's $1.00. Mythos 5.1's
    // cache-read rate was open at launch, so Mythos stays on the Fable 5 row.
    if m.contains("fable-5-1") || m.contains("fable-5.1") {
        return Some(tier(10.0, 50.0, 12.50, 0.25));
    }
    if m.contains("fable") || m.contains("mythos") {
        return Some(tier(10.0, 50.0, 12.50, 1.00));
    }
    if m.contains("opus") {
        // Opus 4.5+ dropped to $5/$25 (was $15/$75 on Opus 3 / 4.0).
        return Some(tier(5.0, 25.0, 6.25, 0.50));
    }
    if m.contains("sonnet") {
        return Some(tier(3.0, 15.0, 3.75, 0.30));
    }
    // --- OpenAI / GPT (no separate cache-write premium) ---
    if m.contains("gpt") {
        if m.contains("5.5") || m.contains("5-5") {
            return Some(tier(5.0, 30.0, 5.0, 0.50));
        }
        if m.contains("5.6") || m.contains("5-6") {
            // Best-effort until GPT-5.6 Sol/Terra/Luna publish distinct API
            // prices. Keep them on the standard GPT tier rather than falling
            // through silently.
            return Some(tier(2.5, 15.0, 2.5, 0.25));
        }
        // GPT standard tier for older/non-premium GPT families, including
        // Codex Spark.
        return Some(tier(2.5, 15.0, 2.5, 0.25));
    }
    // --- Google / Gemini ---
    if m.contains("gemini") {
        if m.contains("pro") {
            return Some(tier(2.0, 12.0, 2.0, 0.20));
        }
        // flash / flash-lite
        return Some(tier(1.5, 9.0, 1.5, 0.15));
    }
    // --- xAI / Grok ---
    if m.contains("grok") {
        return Some(tier(3.0, 15.0, 3.0, 0.20));
    }
    // --- DeepSeek (V4 Pro / Flash; legacy chat/reasoner aliases retained) ---
    if m.contains("deepseek") {
        if m.contains("flash") || m.contains("chat") {
            return Some(tier(0.27, 1.10, 0.27, 0.07));
        }
        if m.contains("v4") || m.contains("pro") || m.contains("reasoner") || m.contains("r1") {
            return Some(tier(0.55, 2.19, 0.55, 0.14));
        }
        return Some(tier(0.27, 1.10, 0.27, 0.07));
    }
    // --- Local models carry no metered cost ---
    if m.contains("ollama") {
        return Some(tier(0.0, 0.0, 0.0, 0.0));
    }
    None
}

#[must_use]
/// Formats a dollar-denominated value for CLI display.
pub fn format_usd(amount: f64) -> String {
    format!("${amount:.4}")
}
