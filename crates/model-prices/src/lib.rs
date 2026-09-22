//! The price of a token, per model — one JSON file, read here.
//!
//! `resources/model-prices.json` is the table and this crate is the lookup.
//! The two vendors price differently (OpenAI: a cached-input rate and a
//! long-context surcharge per request; Anthropic: cache multipliers and a
//! launch price with a date on it), so each has its own row shape, but both
//! are answered the same way: an id as written in a rollout or transcript is
//! normalised, the row whose key fits it best answers, and a model no row
//! names is priced as UNKNOWN — never as free. Teaching this about a release
//! the vendor ships tomorrow is a row in the file, not a rebuild; the file's
//! own `source` fields say where every number came from.
//!
//! Two trees read it: the window's token scanners, which price a message by
//! the day it was sent, and zo's plan scorer, which prices the turn it is
//! about to spend ([`price`]). They live in separate cargo workspaces and this
//! crate is the one thing they share — a second copy of the matcher is how the
//! two would come to disagree about what a turn cost.

use std::sync::OnceLock;

use serde::Deserialize;

const TABLE: &str = include_str!("../resources/model-prices.json");

#[derive(Debug, Deserialize)]
struct Table {
    openai: OpenAiTable,
    anthropic: AnthropicTable,
    typesafe: SystemOneTable,
}

fn table() -> &'static Table {
    static TABLE_ONCE: OnceLock<Table> = OnceLock::new();
    TABLE_ONCE.get_or_init(|| {
        let table: Table = serde_json::from_str(TABLE)
            .expect("resources/model-prices.json is the shape this module reads");
        // Resolve the fields that can be wrong in a way the shape check cannot
        // see, so a bad date fails here and not on the first price it would
        // have answered.
        for row in &table.anthropic.rows {
            let _ = row.intro.as_ref().map(IntroRow::through_date);
        }
        for row in &table.typesafe.rows {
            let _ = row.announced_date();
        }
        table
    })
}

/* ---- OpenAI ------------------------------------------------------------- */

#[derive(Debug, Deserialize)]
struct OpenAiTable {
    long_context_threshold_tokens: u64,
    reasoning_tiers: Vec<String>,
    rows: Vec<OpenAiRow>,
}

#[derive(Debug, Deserialize)]
struct OpenAiRow {
    /// Ids the row prices when equal to the normalised id or a `-`-segment
    /// prefix of it; the longest such id across the table wins.
    #[serde(default)]
    ids: Vec<String>,
    /// Spellings the row prices ONLY when exact — an alias whose prefix would
    /// swallow cheaper siblings.
    #[serde(default)]
    exact: Vec<String>,
    #[serde(flatten)]
    rate: OpenAiRate,
}

/// Dollars per million tokens, and the prices above the long-context
/// threshold when a model has them.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct OpenAiRate {
    pub input: f64,
    pub cached_input: f64,
    pub output: f64,
    #[serde(default)]
    pub long: Option<LongContextRate>,
}

/// The rates above [`openai_long_context_threshold`], in the same units.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct LongContextRate {
    pub input: f64,
    pub cached_input: f64,
    pub output: f64,
}

impl OpenAiRow {
    #[cfg(test)]
    fn key(&self) -> &str {
        self.ids
            .first()
            .or_else(|| self.exact.first())
            .map_or("", String::as_str)
    }

    /// The length of the longest `ids` entry that fits `normalized`, if any.
    fn fit(&self, normalized: &str) -> Option<usize> {
        self.ids
            .iter()
            .filter(|id| segment_prefix(normalized, id))
            .map(String::len)
            .max()
    }
}

impl OpenAiTable {
    /// An id as written in a rollout, reduced to the spelling the rows use.
    ///
    /// Ported from `normalizeModelForPricing` (`codex-model-pricing.ts:108-180`),
    /// including the two shapes an effort tier arrives in — `gpt-5.6-sol(high)`
    /// and `gpt-5.6-sol-high`, up to four stacked. A parenthesised something
    /// that is NOT a tier is an id this table does not understand, rather than
    /// one to guess at.
    fn normalize(&self, model: &str) -> Option<String> {
        let lower = model.trim().to_ascii_lowercase();
        let lower = match lower
            .strip_suffix(')')
            .and_then(|held| held.rsplit_once('('))
        {
            Some((head, tier)) => {
                if !self
                    .reasoning_tiers
                    .iter()
                    .any(|known| known == tier.trim())
                {
                    return None;
                }
                head.to_string()
            }
            None => lower,
        };
        let mut normalized = lower.as_str();
        for _ in 0..4 {
            let Some(shorter) = self
                .reasoning_tiers
                .iter()
                .find_map(|tier| normalized.strip_suffix(&format!("-{tier}")))
            else {
                break;
            };
            normalized = shorter;
        }
        Some(normalized.to_string())
    }

    fn row_of(&self, model: &str) -> Option<&OpenAiRow> {
        let normalized = self.normalize(model)?;
        self.rows
            .iter()
            .filter_map(|row| row.fit(&normalized).map(|len| (len, row)))
            .max_by_key(|(len, _)| *len)
            .map(|(_, row)| row)
            .or_else(|| self.rows.iter().find(|row| row.exact.contains(&normalized)))
    }
}

fn segment_prefix(model: &str, id: &str) -> bool {
    model == id
        || model
            .strip_prefix(id)
            .is_some_and(|rest| rest.starts_with('-'))
}

/// The rate for an OpenAI model id as a rollout spells it, or `None` for a
/// model the table does not price.
pub fn openai_rate(model: &str) -> Option<OpenAiRate> {
    table().openai.row_of(model).map(|row| row.rate)
}

/// Where the vendor's long-context surcharge begins, in tokens of ONE request.
pub fn openai_long_context_threshold() -> u64 {
    table().openai.long_context_threshold_tokens
}

/* ---- Anthropic ---------------------------------------------------------- */

#[derive(Debug, Deserialize)]
struct AnthropicTable {
    cache_read_rate: f64,
    cache_write_5m_rate: f64,
    cache_write_1h_rate: f64,
    rows: Vec<AnthropicRow>,
}

#[derive(Debug, Deserialize)]
struct AnthropicRow {
    /// `claude-<family>-<version>`; the family key the matcher looks for is
    /// the id without its `claude-` prefix.
    ids: Vec<String>,
    input: f64,
    output: f64,
    /// Reading a cached prefix back, as a multiple of `input`, when it is not
    /// the table's usual one.
    #[serde(default)]
    cache_read_rate: Option<f64>,
    #[serde(default)]
    intro: Option<IntroRow>,
    /// Answers only for the strict shapes of the base release — see
    /// [`is_legacy_base`].
    #[serde(default)]
    legacy_base: bool,
    /// Families whose point releases nobody has taught this table about bill
    /// at this row.
    #[serde(default)]
    prices_unknown_members_of: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IntroRow {
    input: f64,
    output: f64,
    /// `YYYY-MM-DD`, the last day the launch price applies to.
    through: String,
}

impl IntroRow {
    fn through_date(&self) -> CivilDate {
        civil_date("intro `through`", &self.through)
    }
}

/// A `YYYY-MM-DD` the table writes under `field`, read into a [`CivilDate`] —
/// the one parser for every date in the file. Panics on a malformed date: the
/// table ships inside the binary, so a bad one is a build defect, and [`table`]
/// resolves every date when it loads so the panic comes first, not on the
/// first price it would have answered.
fn civil_date(field: &str, text: &str) -> CivilDate {
    let mut parts = text.split('-').map(str::parse::<u32>);
    let (Some(Ok(year)), Some(Ok(month)), Some(Ok(day)), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        panic!("resources/model-prices.json: {field} must be YYYY-MM-DD, not {text:?}");
    };
    CivilDate {
        year: i64::from(year),
        month,
        day,
    }
}

/// A calendar date as the table writes one.
///
/// A date rather than a day number on purpose: turning `2026-08-31` into days
/// since the epoch is `zerocode_core::civil`'s job, and this crate is a leaf
/// that both trees compile — a fifth copy of `days_from_civil` is exactly what
/// that module was written to end. Ordered by year, then month, then day,
/// which for a well-formed date is calendar order, so a caller that only needs
/// "is this day still inside the window" never converts at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CivilDate {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

/// Dollars per million tokens for one Anthropic model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnthropicRate {
    pub input: f64,
    pub output: f64,
    /// Reading a cached prefix back, as a multiple of `input`.
    pub cache_read: f64,
    /// A launch price and the last day it covers, when the vendor is running one.
    pub intro: Option<Intro>,
}

/// A launch price, and the last day it applies to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Intro {
    pub input: f64,
    pub output: f64,
    pub through: CivilDate,
}

impl AnthropicRow {
    #[cfg(test)]
    fn key(&self) -> &str {
        self.ids.first().map_or("", String::as_str)
    }

    fn family_keys(&self) -> impl Iterator<Item = &str> {
        self.ids
            .iter()
            .map(|id| id.strip_prefix("claude-").unwrap_or(id))
    }

    /// The length of the longest family key `lower` names, if any.
    fn fit(&self, lower: &str) -> Option<usize> {
        self.family_keys()
            .filter(|key| names(lower, key))
            .map(str::len)
            .max()
    }

    fn rate(&self, table: &AnthropicTable) -> AnthropicRate {
        AnthropicRate {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read_rate.unwrap_or(table.cache_read_rate),
            intro: self.intro.as_ref().map(|intro| Intro {
                input: intro.input,
                output: intro.output,
                through: intro.through_date(),
            }),
        }
    }
}

impl AnthropicTable {
    /// An id as written in a transcript, reduced to a row.
    ///
    /// Ported from `normalizeModelForPricing` (`claude-model-pricing.ts:82-160`):
    /// the longest family key the id names wins, a legacy base release answers
    /// only for its strict shapes, and an unrecognised point release of a
    /// family bills at that family's current row rather than its legacy one.
    fn row_of(&self, model: &str) -> Option<&AnthropicRow> {
        let lower = model.trim().to_ascii_lowercase();
        let lower = lower
            .strip_prefix("anthropic/")
            .or_else(|| lower.strip_prefix("anthropic:"))
            .unwrap_or(&lower)
            .replace('.', "-");
        self.rows
            .iter()
            .filter(|row| !row.legacy_base)
            .filter_map(|row| row.fit(&lower).map(|len| (len, row)))
            .max_by_key(|(len, _)| *len)
            .map(|(_, row)| row)
            .or_else(|| {
                self.rows
                    .iter()
                    .filter(|row| row.legacy_base)
                    .find(|row| row.family_keys().any(|key| is_legacy_base(&lower, key)))
            })
            .or_else(|| {
                self.rows.iter().find(|row| {
                    row.prices_unknown_members_of
                        .iter()
                        .any(|family| lower.contains(family.as_str()))
                })
            })
    }
}

/// Whether `lower` names `key` (`opus-4-8`) as a version, not as the head of a
/// longer one: the key is followed by a non-digit or by nothing.
fn names(lower: &str, key: &str) -> bool {
    lower.find(key).is_some_and(|at| {
        lower[at + key.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_ascii_digit())
    })
}

/// Whether `lower` is the ORIGINAL release `key` names (`opus-4`), which may
/// bill differently from every later point release.
///
/// Orca's `isLegacyBaseOpus4Model` (`claude-model-pricing.ts:75-78`) as a
/// predicate rather than a regex: the key must END the id, or be followed only
/// by `-thinking`, a `-20YYMMDD` date (optionally `-thinking`), or an
/// `@20YYMMDD` snapshot. A looser test — "followed by anything that is not a
/// digit" — swallows a point release that does not exist yet and bills it at
/// the legacy rate; over-charging a future model is the worse failure, so the
/// doubt goes the other way and the family's current row takes it.
fn is_legacy_base(lower: &str, key: &str) -> bool {
    let Some(at) = lower.find(key) else {
        return false;
    };
    let tail = &lower[at + key.len()..];
    let dated = |rest: &str| {
        rest.len() == 8 && rest.starts_with("20") && rest.bytes().all(|byte| byte.is_ascii_digit())
    };
    tail.is_empty()
        || tail == "-thinking"
        || tail
            .strip_prefix('-')
            .is_some_and(|rest| dated(rest) || rest.strip_suffix("-thinking").is_some_and(dated))
        || tail.strip_prefix('@').is_some_and(dated)
}

/// The rate for an Anthropic model id as a transcript spells it, or `None`
/// for a model the table does not price.
pub fn anthropic_rate(model: &str) -> Option<AnthropicRate> {
    let table = &table().anthropic;
    table.row_of(model).map(|row| row.rate(table))
}

/// Writing a prefix that lives five minutes, and one that lives an hour, as
/// multiples of the model's input rate.
pub fn anthropic_cache_write_rates() -> (f64, f64) {
    let table = &table().anthropic;
    (table.cache_write_5m_rate, table.cache_write_1h_rate)
}

/* ---- TypeSafe System One ------------------------------------------------ */

#[derive(Debug, Deserialize)]
struct SystemOneTable {
    rows: Vec<SystemOneRow>,
}

#[derive(Debug, Deserialize)]
struct SystemOneRow {
    ids: Vec<String>,
    /// The family whose numbered versions (`jev-1.13.0` of `jev`) bill at
    /// this row — the one place a pinned version finds its price, since the
    /// vendor names one price per model and a version is the id a pinned
    /// request asks with (t-6187).
    #[serde(default)]
    prices_versions_of: Option<String>,
    input: f64,
    output: f64,
    /// `YYYY-MM-DD`, the day the vendor published this price.
    announced: String,
}

impl SystemOneRow {
    #[cfg(test)]
    fn key(&self) -> &str {
        self.ids.first().map_or("", String::as_str)
    }

    fn announced_date(&self) -> CivilDate {
        civil_date("typesafe `announced`", &self.announced)
    }
}

/// Dollars per million tokens for one System One model — a typed judgment
/// endpoint that bills input only — and the day the price was announced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemOneRate {
    pub input: f64,
    pub output: f64,
    pub announced: CivilDate,
}

/// The table's unit: every rate is dollars per this many tokens.
const TOKENS_PER_RATE_UNIT: f64 = 1_000_000.0;

impl SystemOneRate {
    /// What `input_tokens` cost at this rate. Output is priced too, but a
    /// System One call writes no prose, so input is the bill.
    #[must_use]
    pub fn input_cost_usd(&self, input_tokens: u64) -> f64 {
        // A ledger's token sums stay far below 2^53, where `f64` stops being
        // exact for integers.
        #[allow(clippy::cast_precision_loss)]
        let tokens = input_tokens as f64;
        tokens * self.input / TOKENS_PER_RATE_UNIT
    }
}

/// The rate for a System One model id, or `None` for one no row names. The id
/// must be one a row names, in any case, or a numbered version of the family
/// a row prices (`prices_versions_of`: `jev-1.13.0` is a `jev`): a dated or
/// otherwise named id nobody wrote down is unpriced, never billed at a
/// neighbour's rate.
pub fn systemone_rate(model: &str) -> Option<SystemOneRate> {
    let wanted = model.trim();
    let rows = &table().typesafe.rows;
    rows.iter()
        .find(|row| row.ids.iter().any(|id| id.eq_ignore_ascii_case(wanted)))
        .or_else(|| {
            rows.iter().find(|row| {
                row.prices_versions_of
                    .as_deref()
                    .is_some_and(|family| is_numbered_version_of(wanted, family))
            })
        })
        .map(|row| SystemOneRate {
            input: row.input,
            output: row.output,
            announced: row.announced_date(),
        })
}

/// Whether `id` is a numbered version of `family`, in any case: the family, a
/// hyphen, and whole numbers joined by dots (`jev-1.13.0`, `jev-2`). A date
/// (`jev-2026-09-15`) is hyphenated, and a variant (`jev-1.13.0-preview`)
/// carries a word, so neither is one.
fn is_numbered_version_of(id: &str, family: &str) -> bool {
    id.get(..family.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(family))
        && id[family.len()..].strip_prefix('-').is_some_and(|version| {
            version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        })
}

/* ---- both vendors, one shape ------------------------------------------- */

/// What one model's tokens list for, USD per million, in the four buckets a
/// request is billed in.
///
/// The two vendor tables do not carry these four numbers — Anthropic writes
/// its cache rates once, as multiples of input, and OpenAI writes a cached
/// input rate and no write rate at all — so this is the shape they are folded
/// into, once, here. A caller that wants a vendor's own row (the long-context
/// surcharge, the launch window, the hour-long write) asks [`openai_rate`] or
/// [`anthropic_rate`] instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    /// Reading a prefix back that is already cached.
    pub cache_read: f64,
    /// Writing a prefix into the cache.
    pub cache_write: f64,
    pub output: f64,
}

/// The standing list price of `model`, whichever vendor's table names it, or
/// `None` for a model neither does.
///
/// `None` is "nobody wrote this rate down", never "free": a caller that folds
/// an unpriced model in at zero quietly reports a cheaper plan than it bought.
///
/// Two rates the vendors carry are deliberately NOT folded in here, because
/// neither is a property of the model: OpenAI's long-context surcharge belongs
/// to one request's size, and Anthropic's launch window belongs to the day a
/// message was sent. The one launch row in the table is read through
/// [`anthropic_rate`] by the scanner that prices history; a plan being scored
/// is priced at the standing rate.
///
/// The cache buckets follow each vendor's own arithmetic: Anthropic's read and
/// five-minute write are multiples of input (the table's `cache_read_rate` and
/// `cache_write_5m_rate`, so a row with its own read multiple — Fable 5.1 —
/// keeps it), and OpenAI bills a cached read at `cached_input` and charges no
/// premium at all for the write, so its write rate IS its input rate.
#[must_use]
pub fn price(model: &str) -> Option<Price> {
    if let Some(rate) = openai_rate(model) {
        return Some(Price {
            input: rate.input,
            cache_read: rate.cached_input,
            cache_write: rate.input,
            output: rate.output,
        });
    }
    let rate = anthropic_rate(model)?;
    let (write_5m_rate, _write_1h_rate) = anthropic_cache_write_rates();
    Some(Price {
        input: rate.input,
        cache_read: rate.input * rate.cache_read,
        cache_write: rate.input * write_5m_rate,
        output: rate.output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openai_key(model: &str) -> Option<&'static str> {
        table().openai.row_of(model).map(OpenAiRow::key)
    }

    fn anthropic_key(model: &str) -> Option<&'static str> {
        table().anthropic.row_of(model).map(AnthropicRow::key)
    }

    /// A System One model bills input only and carries the day its price was
    /// announced; it is matched by its exact id, and it is never folded into
    /// the chat-model shape the plan scorer prices candidates in.
    #[test]
    fn a_system_one_model_is_priced_by_its_own_row() {
        let jev = systemone_rate("jev-latest").expect("the launch price is written down");
        assert_eq!((jev.input, jev.output), (0.042, 0.0));
        assert_eq!(
            jev.announced,
            CivilDate {
                year: 2026,
                month: 9,
                day: 15
            }
        );
        assert_eq!(
            systemone_rate(" JEV-LATEST "),
            Some(jev),
            "case and padding are the caller's"
        );
        for unnamed in [
            "jev",
            "jev-2026-09-15",
            "jev-latest-preview",
            "gpt-5.6-sol",
            "",
        ] {
            assert_eq!(systemone_rate(unnamed), None, "{unnamed} was priced");
        }
        assert_eq!(
            price("jev-latest"),
            None,
            "a judgment call is not a chat candidate"
        );
    }

    /// A pinned version of Jev (`smart.jevModel`, t-6187) bills at the
    /// family's own row — the vendor names one price for Jev, and the pin
    /// is the id the seat asks with — while a dated id, a named variant and
    /// anything that is not a numbered version stay unpriced.
    #[test]
    fn a_numbered_version_bills_at_its_familys_row() {
        let family = systemone_rate("jev-latest").expect("the launch price is written down");
        for version in ["jev-1.13.0", " JEV-1.13.0 ", "jev-2", "jev-1.14"] {
            assert_eq!(systemone_rate(version), Some(family), "{version}");
        }
        for unnamed in [
            "jev-",
            "jev-1.",
            "jev-.1",
            "jev-1..2",
            "jev-1.13.0-preview",
            "jev-2026-09-15",
            "jevv-1.13.0",
            "pev-1.13.0",
        ] {
            assert_eq!(systemone_rate(unnamed), None, "{unnamed} was priced");
        }
    }

    /// The table loads, every row has a key, and no key is spelled twice —
    /// a duplicate would make "longest wins" pick by row order.
    #[test]
    fn the_table_loads_and_every_row_is_named_once() {
        let table = table();
        let mut keys: Vec<&str> = table.openai.rows.iter().map(OpenAiRow::key).collect();
        keys.extend(table.anthropic.rows.iter().map(AnthropicRow::key));
        keys.extend(table.typesafe.rows.iter().map(SystemOneRow::key));
        assert!(
            keys.iter().all(|key| !key.is_empty()),
            "a row without a key"
        );
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "a key spelled twice: {keys:?}");
        assert!(
            table
                .anthropic
                .rows
                .iter()
                .all(|row| row.ids.iter().all(|id| id.starts_with("claude-"))),
            "an Anthropic row whose id is not claude-<family>-<version>"
        );
        assert_eq!(openai_long_context_threshold(), 272_000);
        assert_eq!(anthropic_cache_write_rates(), (1.25, 2.0));
    }

    /// The OpenAI matcher answers what the hand-written ladder answered: the
    /// longest family prefix, effort tiers stripped in both spellings, and the
    /// two aliases priced only when spelled exactly.
    #[test]
    fn openai_ids_resolve_to_the_rows_the_ladder_chose() {
        let cases = [
            ("gpt-5", Some("gpt-5")),
            ("gpt-5-codex", Some("gpt-5")),
            ("gpt-5-codex-high", Some("gpt-5")),
            ("gpt-5-turbo", None),
            ("gpt-5.1-codex-max-high", Some("gpt-5.1-codex-max")),
            ("gpt-5.1-codex", Some("gpt-5.1-codex")),
            ("gpt-5.1", Some("gpt-5.1")),
            ("gpt-5.3-codex-spark", Some("gpt-5.3-codex-spark")),
            ("gpt-5.4-mini(low)", Some("gpt-5.4-mini")),
            ("GPT-5.4-Pro", Some("gpt-5.4-pro")),
            ("gpt-5.5-pro-2026-06-01", Some("gpt-5.5-pro")),
            ("gpt-5.6", Some("gpt-5.6-sol")),
            ("gpt-5.6-sol-xhigh-auto", Some("gpt-5.6-sol")),
            ("gpt-5.6-luna", Some("gpt-5.6-luna")),
            ("gpt-5.6-sol(turbo)", None),
            ("gpt-5.6-sol(high)", Some("gpt-5.6-sol")),
            ("codex-auto-review", None),
            ("gpt-9", None),
            ("", None),
        ];
        for (model, expected) in cases {
            assert_eq!(openai_key(model), expected, "{model}");
        }
        let sol = openai_rate("gpt-5.6-sol").expect("priced");
        assert_eq!(sol.input, 5.0);
        assert_eq!(sol.long.map(|long| long.output), Some(45.0));
        assert!(openai_rate("gpt-5.3-codex").expect("priced").long.is_none());
    }

    /// The Anthropic matcher answers what the hand-written ladder answered:
    /// the longest family key named, the legacy base release only in its
    /// strict shapes, and an unknown point release at the family's current row.
    #[test]
    fn anthropic_ids_resolve_to_the_rows_the_ladder_chose() {
        let cases = [
            ("claude-fable-5-1", Some("claude-fable-5-1")),
            (
                "anthropic/claude-fable-5-1-20260828",
                Some("claude-fable-5-1"),
            ),
            ("claude-fable-5.1", Some("claude-fable-5-1")),
            ("claude-fable-5", Some("claude-fable-5")),
            ("claude-opus-5-20260101", Some("claude-opus-5")),
            ("claude-opus-4-8", Some("claude-opus-4-8")),
            ("claude-opus-4.8", Some("claude-opus-4-8")),
            ("claude-opus-4-8-thinking", Some("claude-opus-4-8")),
            ("claude-opus-4-9", Some("claude-opus-4-8")),
            ("claude-opus-4-1", Some("claude-opus-4-1")),
            ("claude-opus-4", Some("claude-opus-4")),
            ("claude-opus-4-20250514", Some("claude-opus-4")),
            ("claude-opus-4-thinking", Some("claude-opus-4")),
            ("claude-opus-4@20250514", Some("claude-opus-4")),
            ("anthropic:claude-sonnet-5", Some("claude-sonnet-5")),
            ("claude-sonnet-4-6", Some("claude-sonnet-4-6")),
            ("claude-sonnet-4-9", Some("claude-sonnet-4-6")),
            ("claude-haiku-4-5-20251001", Some("claude-haiku-4-5")),
            ("<synthetic>", None),
            ("gpt-4", None),
            ("", None),
        ];
        for (model, expected) in cases {
            assert_eq!(anthropic_key(model), expected, "{model}");
        }
        let fable = anthropic_rate("claude-fable-5-1").expect("priced");
        assert_eq!(
            fable.cache_read, 0.025,
            "the one row that reads at a fortieth"
        );
        assert_eq!(
            anthropic_rate("claude-fable-5").expect("priced").cache_read,
            0.1
        );
        let sonnet = anthropic_rate("claude-sonnet-5").expect("priced");
        let intro = sonnet.intro.expect("a launch price");
        assert_eq!((intro.input, intro.output), (2.0, 10.0));
        assert_eq!(
            intro.through,
            CivilDate {
                year: 2026,
                month: 8,
                day: 31
            }
        );
        assert!(
            anthropic_rate("claude-opus-5")
                .expect("priced")
                .intro
                .is_none()
        );
    }

    /// The one shape both vendors fold into: Anthropic's cache buckets are its
    /// input rate times the table's multiples (with a row's own read multiple
    /// kept), OpenAI reads at `cached_input` and writes at no premium, and a
    /// model neither table names is unpriced rather than free.
    #[test]
    fn one_price_shape_folds_each_vendors_own_arithmetic() {
        let opus = price("claude-opus-5").expect("priced");
        assert_eq!(
            opus,
            Price {
                input: 5.0,
                cache_read: 0.5,
                cache_write: 6.25,
                output: 25.0
            }
        );
        let fable = price("claude-fable-5-1").expect("priced");
        assert_eq!(
            (fable.input, fable.cache_read, fable.cache_write),
            (10.0, 0.25, 12.5),
            "the row with its own read multiple keeps it"
        );
        let sol = price("gpt-5.6-sol").expect("priced");
        assert_eq!(
            sol,
            Price {
                input: 5.0,
                cache_read: 0.5,
                cache_write: 5.0,
                output: 30.0
            },
            "no write premium: the write rate IS the input rate"
        );
        for unknown in ["gemini-3-pro", "grok-5", "<synthetic>", ""] {
            assert_eq!(price(unknown), None, "{unknown} was priced");
        }
    }
}
