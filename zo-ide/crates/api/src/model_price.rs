//! What a model's tokens list for — zo's door onto the shared price table.
//!
//! The table and its matcher live in `crates/model-prices` at the top of this
//! repository, next to the window that has read it since 2026-09-05. zo does
//! not carry a second copy: the plan scorer and the window's token scanners
//! have to agree about what a turn cost, and two tables would be two answers
//! (`docs/design/zo-autonomous-routing-review-20260915.md` §4.2).
//!
//! The catalog is the reason this sits in `api` rather than in the scorer:
//! the ids the scorer ranks are catalog ids, and the catalog is this crate's.

pub use model_prices::{CivilDate, Price as ModelPrice, SystemOneRate};

/// The list price of a model id, USD per million tokens, or `None` when no row
/// names it.
///
/// `None` means "nobody wrote this rate down", never "free" — an unpriced
/// candidate ranks last in the scorer rather than cheapest.
#[must_use]
pub fn model_price(id: &str) -> Option<ModelPrice> {
    model_prices::price(id)
}

/// The list price of a System One model — a typed judgment endpoint, billed
/// input only — or `None` when no row names it. Kept apart from
/// [`model_price`]: a judgment call is never a chat candidate the plan scorer
/// could rank.
#[must_use]
pub fn systemone_rate(id: &str) -> Option<SystemOneRate> {
    model_prices::systemone_rate(id)
}

#[cfg(test)]
mod tests {
    use super::model_price;

    /// Anthropic's table writes one input rate and defines the cache buckets
    /// as multiples of it. What comes out the door is the multiplication, with
    /// the row's own read multiple where it has one.
    #[test]
    fn an_anthropic_model_prices_its_cache_at_the_tables_multiples() {
        let opus = model_price("claude-opus-5").expect("the table prices Opus 5");
        assert!((opus.input - 5.0).abs() < f64::EPSILON, "{opus:?}");
        assert!(
            (opus.cache_read - 0.5).abs() < f64::EPSILON,
            "a read is a tenth of input: {opus:?}"
        );
        assert!(
            (opus.cache_write - 6.25).abs() < f64::EPSILON,
            "a five-minute write is 1.25x input: {opus:?}"
        );
        assert!((opus.output - 25.0).abs() < f64::EPSILON, "{opus:?}");

        let fable = model_price("claude-fable-5-1").expect("the table prices Fable 5.1");
        assert!(
            (fable.cache_read - 0.25).abs() < f64::EPSILON,
            "the one row that reads at a fortieth keeps its own multiple: {fable:?}"
        );
    }

    #[test]
    fn openai_rates_include_the_published_cache_write_premium() {
        let price = model_price("gpt-5.6-terra").expect("published model rates");
        assert!((price.input - 2.0).abs() < f64::EPSILON, "{price:?}");
        assert!((price.cache_read - 0.20).abs() < f64::EPSILON, "{price:?}");
        assert!((price.cache_write - 2.50).abs() < f64::EPSILON, "{price:?}");
        assert!((price.output - 12.0).abs() < f64::EPSILON, "{price:?}");
    }

    /// A model no row names is unpriced. Zero would be a lie the scorer would
    /// rank first.
    #[test]
    fn a_model_the_table_does_not_name_is_unpriced_rather_than_free() {
        for unknown in [
            "gemini-3-pro",
            "grok-5",
            "qwen3-coder",
            "<synthetic>",
            "",
        ] {
            assert!(
                model_price(unknown).is_none(),
                "{unknown} came back with a price"
            );
        }
    }
}
