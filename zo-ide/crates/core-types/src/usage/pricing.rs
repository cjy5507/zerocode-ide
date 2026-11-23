//! Per-million-token pricing data, model→pricing lookup, the derived cost
//! estimate type, and dollar formatting for the usage ledger.

/// Per-million-token pricing used for cost estimation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub cache_creation_cost_per_million: f64,
    pub cache_read_cost_per_million: f64,
}

pub use model_prices::Cost as UsageCostEstimate;

/// Public list rates for the model ID recorded by the provider.
#[must_use]
pub fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    pricing_at(model, model_prices::today())
}

pub(super) fn pricing_at(model: &str, day: i64) -> Option<ModelPricing> {
    model_prices::price_at(model, day).map(|price| ModelPricing {
        input_cost_per_million: price.input,
        output_cost_per_million: price.output,
        cache_creation_cost_per_million: price.cache_write,
        cache_read_cost_per_million: price.cache_read,
    })
}

#[must_use]
/// Formats a dollar-denominated value for CLI display.
pub fn format_usd(amount: f64) -> String {
    format!("${amount:.4}")
}
