//! Public token rates used by both the window and zo.
//!
//! The catalog names exact IDs and aliases and dates each rate. Raw usage
//! retains its model ID for display, even when the catalog has no price.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const DATA: &str = include_str!("../resources/model-prices.json");
const SECONDS_PER_DAY: u64 = 86_400;

#[derive(Debug, Deserialize)]
struct Catalog {
    reasoning_tiers: Vec<String>,
    rows: Vec<RateRow>,
    typesafe: Vec<SystemOneRow>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(DATA).expect("embedded token price catalog"))
}

#[derive(Debug, Deserialize)]
struct RateRow {
    model: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(deserialize_with = "read_day")]
    effective_from: i64,
    #[serde(default, deserialize_with = "read_optional_day")]
    effective_through: Option<i64>,
    rates: Rates,
    long_context: Option<LongContext>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Rates {
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    cache_write_1h: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct LongContext {
    threshold_tokens: u64,
    rates: Rates,
}

impl Catalog {
    fn row(&self, model: &str, day: Option<i64>) -> Option<&RateRow> {
        let normalized = normalize(model, &self.reasoning_tiers);
        let mut id = normalized.as_str();
        for _ in 0..=4 {
            let mut matches = self
                .rows
                .iter()
                .filter(|row| row.model == id || row.aliases.iter().any(|alias| alias == id))
                .peekable();
            if matches.peek().is_some() {
                return matches
                    .filter(|row| {
                        day.is_none_or(|day| {
                            row.effective_from <= day
                                && row.effective_through.is_none_or(|end| day <= end)
                        })
                    })
                    .max_by_key(|row| row.effective_from);
            }
            let (head, tier) = id.rsplit_once('-')?;
            if tier != "thinking" && !self.reasoning_tiers.iter().any(|known| known == tier) {
                break;
            }
            id = head;
        }
        None
    }
}

fn normalize(model: &str, reasoning_tiers: &[String]) -> String {
    let lower = model.trim().to_ascii_lowercase();
    let model = ["openai/", "openai:", "anthropic/", "anthropic:"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))
        .unwrap_or(&lower);
    let id = model
        .strip_suffix(')')
        .and_then(|value| value.rsplit_once('('))
        .filter(|(_, tier)| reasoning_tiers.iter().any(|known| known == tier.trim()))
        .map_or(model, |(id, _)| id.trim_end());
    // Claude transcripts can spell version separators as dots.
    if id.starts_with("claude-") {
        id.replace('.', "-")
    } else {
        id.to_owned()
    }
}

fn read_day<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let value = String::deserialize(deserializer)?;
    pricing_day(&value).ok_or_else(|| serde::de::Error::custom("expected a pricing date"))
}

fn read_optional_day<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    read_day(deserializer).map(Some)
}

/// A UTC calendar date, expressed in days since 1970-01-01.
#[must_use]
pub fn pricing_day(value: &str) -> Option<i64> {
    let (year, month, day) = date_parts(value.get(..10)?)?;
    Some(days_from_civil(year, month, day))
}

fn date_parts(date: &str) -> Option<(i64, u32, u32)> {
    let (year, rest) = date.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    let year: i64 = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    let day: u32 = day.parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some((year, month, day))
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
#[must_use]
pub const fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = month as i64;
    let day = day as i64;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Convert a Unix millisecond timestamp to its UTC pricing day.
#[must_use]
pub const fn day_at_millis(ms: i64) -> i64 {
    ms.div_euclid(86_400_000)
}

/// The current UTC pricing day.
#[must_use]
pub fn today() -> i64 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    i64::try_from(seconds / SECONDS_PER_DAY).unwrap_or(i64::MAX)
}

/// Usage buckets are exclusive. Providers that include cache reads in
/// total input must subtract those reads before calling this crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct RequestTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Total cache writes, including any hour-long writes.
    pub cache_write: u64,
    pub cache_write_1h: u64,
}

impl RequestTokens {
    /// Input including cache reads and writes.
    #[must_use]
    pub fn prompt_tokens(self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}

/// Estimated USD for a priced usage sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cost {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub cache_creation_cost_usd: f64,
    pub cache_read_cost_usd: f64,
}

impl Cost {
    #[must_use]
    pub fn total_cost_usd(self) -> f64 {
        self.input_cost_usd
            + self.output_cost_usd
            + self.cache_creation_cost_usd
            + self.cache_read_cost_usd
    }
}

/// A subtotal of known costs and the number of requests it excludes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CostTotal {
    pub cost_usd: Option<f64>,
    pub unpriced_requests: u64,
}

impl CostTotal {
    pub fn record(&mut self, cost: Option<f64>) {
        self.cost_usd = sum_cost(self.cost_usd, cost);
        self.unpriced_requests += u64::from(cost.is_none());
    }

    pub fn merge(&mut self, total: Self) {
        self.cost_usd = sum_cost(self.cost_usd, total.cost_usd);
        self.unpriced_requests += total.unpriced_requests;
    }

    /// Older serialized summaries did not record per-request prices.
    #[must_use]
    pub fn for_requests(mut self, count: i64) -> Self {
        if self.cost_usd.is_none() && self.unpriced_requests == 0 {
            self.unpriced_requests = u64::try_from(count).unwrap_or(0);
        }
        self
    }

    #[must_use]
    pub fn complete_cost(self) -> Option<f64> {
        if self.unpriced_requests == 0 {
            self.cost_usd
        } else {
            None
        }
    }
}

#[must_use]
pub fn sum_cost(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left + right),
        (left, right) => left.or(right),
    }
}

impl RateRow {
    fn cost(&self, tokens: RequestTokens, single_request: bool) -> Option<Cost> {
        let mut rates = self.rates;
        if let Some(long) = &self.long_context
            && tokens.prompt_tokens() > long.threshold_tokens
        {
            // A sum of requests cannot establish which crossed the threshold.
            if !single_request {
                return None;
            }
            rates = long.rates;
        }
        let hour = tokens.cache_write_1h.min(tokens.cache_write);
        Some(Cost {
            input_cost_usd: token_cost(tokens.input, rates.input),
            output_cost_usd: token_cost(tokens.output, rates.output),
            cache_creation_cost_usd: bucket_cost(tokens.cache_write - hour, rates.cache_write)?
                + bucket_cost(hour, rates.cache_write_1h)?,
            cache_read_cost_usd: bucket_cost(tokens.cache_read, rates.cache_read)?,
        })
    }
}

fn bucket_cost(tokens: u64, rate: Option<f64>) -> Option<f64> {
    if tokens == 0 {
        Some(0.0)
    } else {
        rate.map(|rate| token_cost(tokens, rate))
    }
}

#[allow(clippy::cast_precision_loss)]
fn token_cost(tokens: u64, per_million: f64) -> f64 {
    tokens as f64 * per_million / 1_000_000.0
}

/// Price one request on the day it ran.
#[must_use]
pub fn request_cost(model: &str, day: i64, tokens: RequestTokens) -> Option<Cost> {
    catalog().row(model, Some(day))?.cost(tokens, true)
}

/// Price aggregated counters when the data is precise enough to select a rate.
#[must_use]
pub fn aggregate_cost(model: &str, day: i64, tokens: RequestTokens) -> Option<Cost> {
    catalog().row(model, Some(day))?.cost(tokens, false)
}

/// Standard USD per million tokens for a prospective request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
}

#[must_use]
pub fn price_at(model: &str, day: i64) -> Option<Price> {
    let rates = catalog().row(model, Some(day))?.rates;
    Some(Price {
        input: rates.input,
        cache_read: rates.cache_read.unwrap_or(rates.input),
        cache_write: rates.cache_write.unwrap_or(rates.input),
        output: rates.output,
    })
}

#[must_use]
pub fn price(model: &str) -> Option<Price> {
    price_at(model, today())
}

/// Cache read, short write, and one-hour write as multiples of input.
/// The cache planner needs all three for a meaningful comparison.
#[must_use]
pub fn cache_multipliers(model: &str) -> Option<(f64, f64, f64)> {
    let rates = catalog().row(model, Some(today()))?.rates;
    if rates.input <= 0.0 {
        return None;
    }
    Some((
        rates.cache_read? / rates.input,
        rates.cache_write? / rates.input,
        rates.cache_write_1h? / rates.input,
    ))
}

/// The catalog ID, for callers that need to group equivalent spellings.
#[must_use]
pub fn model_key(model: &str) -> Option<&'static str> {
    catalog().row(model, None).map(|row| row.model.as_str())
}

#[derive(Debug, Deserialize)]
struct SystemOneRow {
    ids: Vec<String>,
    prices_versions_of: Option<String>,
    input: f64,
    output: f64,
    effective_from: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilDate {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

/// Dollars per million tokens for one System One model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemOneRate {
    pub input: f64,
    pub output: f64,
    pub announced: CivilDate,
}

impl SystemOneRate {
    #[must_use]
    pub fn cost_usd(&self, input_tokens: u64, output_tokens: u64) -> f64 {
        self.input_cost_usd(input_tokens) + token_cost(output_tokens, self.output)
    }

    #[must_use]
    pub fn input_cost_usd(&self, input_tokens: u64) -> f64 {
        token_cost(input_tokens, self.input)
    }
}

/// The rate announced for a System One model or a numbered version covered
/// by the provider's published rate.
#[must_use]
pub fn systemone_rate(model: &str) -> Option<SystemOneRate> {
    let wanted = model.trim();
    let rows = &catalog().typesafe;
    let row = rows.iter().find(|row| {
        row.ids.iter().any(|id| id.eq_ignore_ascii_case(wanted))
            || row
                .prices_versions_of
                .as_deref()
                .is_some_and(|family| is_numbered_version_of(wanted, family))
    })?;
    let (year, month, day) = date_parts(&row.effective_from)?;
    Some(SystemOneRate {
        input: row.input,
        output: row.output,
        announced: CivilDate { year, month, day },
    })
}

fn is_numbered_version_of(id: &str, family: &str) -> bool {
    id.get(..family.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(family))
        && id[family.len()..].strip_prefix('-').is_some_and(|version| {
            version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        })
}

#[cfg(test)]
mod pricing_contract {
    use super::*;

    #[test]
    fn an_unlisted_successor_does_not_inherit_the_parent_price() {
        assert!(model_key("gpt-5.4-unlisted").is_none());
        assert!(model_key("claude-opus-4-867").is_none());
    }

    #[test]
    fn a_new_model_id_is_read_from_data() {
        let mut data: serde_json::Value = serde_json::from_str(DATA).unwrap();
        data["rows"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "model": "synthetic-provider-model",
                "aliases": ["synthetic-cli-model"],
                "source": "https://prices.example.test",
                "verified_on": "2026-11-23",
                "effective_from": "2026-01-01",
                "rates": {"input": 0.0, "output": 0.0}
            }));
        let catalog: Catalog = serde_json::from_value(data).unwrap();
        let day = pricing_day("2026-11-23").unwrap();
        let tokens = RequestTokens {
            input: 1_000,
            ..RequestTokens::default()
        };
        let free = catalog
            .row("synthetic-cli-model", Some(day))
            .unwrap()
            .cost(tokens, true);
        assert_eq!(free.map(Cost::total_cost_usd), Some(0.0));
        assert!(
            catalog
                .row("synthetic-provider-model-next", Some(day))
                .is_none()
        );
    }

    #[test]
    fn each_rate_has_an_official_source_and_an_effective_date() {
        let data: serde_json::Value = serde_json::from_str(DATA).unwrap();
        for (key, domains) in [
            (
                "rows",
                &[
                    "developers.openai.com",
                    "openai.com",
                    "platform.claude.com",
                    "www.anthropic.com",
                    "www-cdn.anthropic.com",
                    "ai.google.dev",
                    "api-docs.deepseek.com",
                    "docs.x.ai",
                ][..],
            ),
            ("typesafe", &["typesafe.ai"][..]),
        ] {
            for row in data[key].as_array().unwrap() {
                let host = row["source"]
                    .as_str()
                    .and_then(|url| url.strip_prefix("https://"))
                    .and_then(|url| url.split('/').next())
                    .unwrap();
                assert!(domains.contains(&host), "{row}");
                let effective = pricing_day(row["effective_from"].as_str().unwrap()).unwrap();
                let verified = pricing_day(row["verified_on"].as_str().unwrap()).unwrap();
                assert!(effective <= verified);
            }
        }
    }

    #[test]
    fn historical_rates_and_request_boundaries_follow_provider_terms() {
        let cost = |model, date, tokens| {
            request_cost(model, pricing_day(date).unwrap(), tokens)
                .unwrap()
                .total_cost_usd()
        };
        let prompt = RequestTokens {
            input: 1_000,
            ..RequestTokens::default()
        };
        assert!((cost("gpt-5.6-terra", "2026-07-29", prompt) - 0.0025).abs() < 1e-10);
        assert!((cost("gpt-5.6-terra", "2026-07-30", prompt) - 0.002).abs() < 1e-10);
        let long = RequestTokens {
            input: 200_000,
            cache_read: 72_001,
            output: 1_000,
            ..RequestTokens::default()
        };
        assert!((cost("gpt-5.4", "2026-11-23", long) - 1.0585005).abs() < 1e-10);
        assert!(aggregate_cost("gpt-5.4", pricing_day("2026-11-23").unwrap(), long).is_none());

        let writes = RequestTokens {
            cache_write: 2_000,
            cache_write_1h: 1_000,
            ..RequestTokens::default()
        };
        assert!((cost("claude-sonnet-4-6", "2026-11-23", writes) - 0.00975).abs() < 1e-10);
    }

    #[test]
    fn systemone_output_is_free_and_numbered_versions_follow_the_published_rate() {
        let rate = systemone_rate("JEV-1.13.0").unwrap();
        assert!((rate.cost_usd(1_000_000, 50_000) - 0.042).abs() < 1e-10);
        assert!(systemone_rate("jev-1.13-preview").is_none());
    }
}
