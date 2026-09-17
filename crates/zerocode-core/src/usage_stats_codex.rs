//! The token ledger for Codex, whose rollouts count differently.
//!
//! [`crate::usage_stats`] reads Claude's transcripts, where each assistant row
//! carries the tokens THAT row spent. Codex writes the other shape: a
//! `token_count` event carries the session's RUNNING TOTAL, and the billable
//! increment has to be recovered from it. Summing those totals would charge a
//! long session once for every event it ever wrote — the first mistake anyone
//! makes with this format, and one that looks plausible on screen because the
//! number is merely large rather than absurd.
//!
//! ## What is measured, and against what
//!
//! The record shape, the delta resolution, the event key, the aggregation
//! keys and the price table are read out of Orca's own
//! `src/main/codex-usage/*` (1.4.184) — `codex-usage-record-parser.ts`,
//! `codex-usage-token-delta.ts`, `codex-model-pricing.ts`,
//! `codex-usage-cost-estimate.ts`. The FILE FORMAT was verified against real
//! `~/.codex/sessions/**/rollout-*.jsonl` on this machine.
//!
//! ## What is here and what is next door
//!
//! This module READS rollouts and PRICES models. The rollup those turns go
//! through — sessions, per-day rows, scope, range, totals — is
//! [`crate::usage_ledger`], shared with the other vendor that reports the same
//! five counters. The names it needs are re-exported at the bottom so a caller
//! still asks this module for the Codex ledger's whole answer.
//!
//! ## The buckets are not Claude's
//!
//! Codex reports input, CACHED input (a subset of input, not a fifth bucket),
//! output, and reasoning output (already billed inside output). So the price
//! charges uncached input at the full rate, the cached part at the cache rate,
//! and output once — and reasoning is shown for visibility while costing
//! nothing extra, which is the original's own note under those cards.

use std::collections::HashSet;

use crate::usage_stats::{Range, Scope};

/// The four counters a `token_count` record carries, plus its own total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

impl RawUsage {
    /// The size of a reading, ignoring the derived total.
    fn magnitude(self) -> i64 {
        self.input_tokens
            + self.cached_input_tokens
            + self.output_tokens
            + self.reasoning_output_tokens
    }

    /// Whether two readings describe the same point in a session.
    ///
    /// The derived total is left out on purpose: an older log can omit it, and
    /// two readings that agree on all four real counters ARE the same reading
    /// whatever a synthesized total says.
    fn same_point(self, other: Self) -> bool {
        self.input_tokens == other.input_tokens
            && self.cached_input_tokens == other.cached_input_tokens
            && self.output_tokens == other.output_tokens
            && self.reasoning_output_tokens == other.reasoning_output_tokens
    }

    fn is_monotonic_after(self, previous: Self) -> bool {
        self.input_tokens >= previous.input_tokens
            && self.cached_input_tokens >= previous.cached_input_tokens
            && self.output_tokens >= previous.output_tokens
            && self.reasoning_output_tokens >= previous.reasoning_output_tokens
    }

    fn minus(self, previous: Self) -> Self {
        Self {
            input_tokens: (self.input_tokens - previous.input_tokens).max(0),
            cached_input_tokens: (self.cached_input_tokens - previous.cached_input_tokens).max(0),
            output_tokens: (self.output_tokens - previous.output_tokens).max(0),
            reasoning_output_tokens: (self.reasoning_output_tokens
                - previous.reasoning_output_tokens)
                .max(0),
            total_tokens: (self.total_tokens - previous.total_tokens).max(0),
        }
    }

    fn plus(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens + other.input_tokens,
            cached_input_tokens: self.cached_input_tokens + other.cached_input_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens + other.reasoning_output_tokens,
            total_tokens: self.total_tokens + other.total_tokens,
        }
    }

    fn is_empty(self) -> bool {
        self.magnitude() == 0 && self.total_tokens == 0
    }
}

/// Reads one usage object, or `None` when the field was absent or not one.
///
/// A missing `total_tokens` is synthesized as input + output rather than
/// input + output + reasoning: reasoning is already billed inside output, and
/// adding it would count it twice in the one figure people compare.
#[must_use]
pub fn read_usage(value: Option<&serde_json::Value>) -> Option<RawUsage> {
    let record = value?.as_object()?;
    let number = |name: &str| {
        record
            .get(name)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    };
    let input_tokens = number("input_tokens");
    let cached_input_tokens = record
        .get("cached_input_tokens")
        .or_else(|| record.get("cache_read_input_tokens"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let output_tokens = number("output_tokens");
    let total = number("total_tokens");
    Some(RawUsage {
        input_tokens,
        cached_input_tokens,
        output_tokens,
        reasoning_output_tokens: number("reasoning_output_tokens"),
        total_tokens: if total > 0 {
            total
        } else {
            input_tokens + output_tokens
        },
    })
}

/// What one `token_count` record turned out to be worth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delta {
    /// A billable increment, and the running total to carry forward.
    Spent {
        delta: RawUsage,
        next_totals: Option<RawUsage>,
    },
    /// Not an increment — only a new baseline to measure the next one from.
    /// A resumed or compacted session restarts its totals, and reading that
    /// restart as spend would invent a whole session's worth of tokens.
    Baseline { next_totals: RawUsage },
}

/// Whether a total that went DOWN is a stale duplicate rather than a restart.
///
/// After compaction the totals genuinely reset to something small; a stale
/// record instead reports nearly what the previous one did. Two tests for
/// "nearly": within 2% of the previous total, or close enough that one more
/// increment of the last size would reach it.
fn looks_stale(current: RawUsage, previous: RawUsage, last: RawUsage) -> bool {
    let (current, previous, last) = (current.magnitude(), previous.magnitude(), last.magnitude());
    if previous <= 0 || current <= 0 || last <= 0 {
        return false;
    }
    current * 100 >= previous * 98 || current + last * 2 >= previous
}

/// Recovers the billable increment from a running total.
///
/// The order matters and is the original's: when the record carries BOTH a
/// total and a `last_token_usage`, the last is the increment and the total is
/// only the new baseline — because after a compaction or a resume the totals
/// are a mutable snapshot and their difference is not what was spent.
#[must_use]
pub fn resolve_delta(
    total: Option<RawUsage>,
    last: Option<RawUsage>,
    previous: Option<RawUsage>,
) -> Option<Delta> {
    match (total, last, previous) {
        (Some(total), Some(last), Some(previous)) => {
            if total.same_point(previous) {
                return None;
            }
            if !total.is_monotonic_after(previous) && looks_stale(total, previous, last) {
                return None;
            }
            Some(Delta::Spent {
                delta: last,
                next_totals: Some(total),
            })
        }
        (Some(total), Some(last), None) => Some(Delta::Spent {
            delta: last,
            next_totals: Some(total),
        }),
        (Some(total), None, Some(previous)) => {
            if total.same_point(previous) {
                return None;
            }
            if !total.is_monotonic_after(previous) {
                return Some(Delta::Baseline { next_totals: total });
            }
            Some(Delta::Spent {
                delta: total.minus(previous),
                next_totals: Some(total),
            })
        }
        (Some(total), None, None) => Some(Delta::Spent {
            delta: total,
            next_totals: Some(total),
        }),
        (None, Some(last), Some(previous)) => Some(Delta::Spent {
            delta: last,
            next_totals: Some(previous.plus(last)),
        }),
        (None, Some(last), None) => Some(Delta::Spent {
            delta: last,
            next_totals: None,
        }),
        (None, None, _) => None,
    }
}

/// The identity a record is deduped by.
///
/// A fork or resume copies `token_count` records byte for byte into a new
/// rollout while rewriting `session_meta.id`, so the session cannot be part of
/// the key — only the record's own fields can.
#[must_use]
pub fn event_key(timestamp: &str, total: Option<RawUsage>, last: Option<RawUsage>) -> String {
    let tuple = |usage: Option<RawUsage>| match usage {
        None => String::new(),
        Some(one) => format!(
            "{},{},{},{},{}",
            one.input_tokens,
            one.cached_input_tokens,
            one.output_tokens,
            one.reasoning_output_tokens,
            one.total_tokens
        ),
    };
    format!("{timestamp}|{}|{}", tuple(total), tuple(last))
}

/// One billable increment, as the rollout recorded it.
///
/// The shape is the shared rollup's, so a read rollout goes straight into
/// [`crate::usage_ledger::aggregate`] with nothing in between to convert. The
/// name stays because a reader of this module is reading about rollout
/// records, and that is what these are.
pub type Event = crate::usage_ledger::Entry;

/// What the reader carries between lines of one rollout.
///
/// A rollout is a conversation, not a list of independent rows: the session's
/// id, its directory and its model are announced once and apply until they are
/// announced again, and the running total is the whole point of the format.
#[derive(Clone, Debug, Default)]
pub struct ReadState {
    pub session_id: String,
    pub session_cwd: Option<String>,
    pub current_cwd: Option<String>,
    pub current_model: Option<String>,
    pub previous_totals: Option<RawUsage>,
}

/// The model named anywhere a record is known to put one.
fn read_model(payload: &serde_json::Value) -> Option<String> {
    let text = |value: Option<&serde_json::Value>, name: &str| {
        value
            .and_then(|value| value.get(name))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let direct = text(Some(payload), "model").or_else(|| text(Some(payload), "model_name"));
    if direct.is_some() {
        return direct;
    }
    let info = payload.get("info");
    let in_info = text(info, "model").or_else(|| text(info, "model_name"));
    if in_info.is_some() {
        return in_info;
    }
    let in_info_metadata = text(info.and_then(|info| info.get("metadata")), "model");
    if in_info_metadata.is_some() {
        return in_info_metadata;
    }
    text(payload.get("metadata"), "model")
}

/// Reads one line of a rollout, updating `state` and answering with the
/// increment when the line was a billable one.
///
/// Three of the record types are not spend but CONTEXT — the session's own
/// header, and the turn headers that move the working directory or change the
/// model. They return `None` while changing what the next increment will be
/// attributed to, which is why this is a reader with state rather than a pure
/// per-line parse.
pub fn read_record(line: &str, state: &mut ReadState) -> Option<Event> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = value.get("type").and_then(serde_json::Value::as_str)?;
    let payload = value.get("payload")?;

    if kind == "session_meta" {
        if let Some(id) = payload.get("id").and_then(serde_json::Value::as_str) {
            state.session_id = id.to_string();
        }
        state.session_cwd = payload
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        if state.current_cwd.is_none() {
            state.current_cwd = state.session_cwd.clone();
        }
        return None;
    }

    if kind == "turn_context" {
        if let Some(cwd) = payload.get("cwd").and_then(serde_json::Value::as_str) {
            state.current_cwd = Some(cwd.to_string());
        } else if state.current_cwd.is_none() {
            state.current_cwd = state.session_cwd.clone();
        }
        if let Some(model) = read_model(payload) {
            state.current_model = Some(model);
        }
        return None;
    }

    if kind != "event_msg"
        || payload.get("type").and_then(serde_json::Value::as_str) != Some("token_count")
    {
        return None;
    }
    let timestamp = value.get("timestamp").and_then(serde_json::Value::as_str)?;
    // A `token_count` with no `info` is a rate-limit ping, not usage. Treating
    // it as malformed would make a healthy session look like a scan error.
    let info = payload.get("info").filter(|info| info.is_object())?;

    let total = read_usage(info.get("total_token_usage"));
    let last = read_usage(info.get("last_token_usage"));
    let resolved = resolve_delta(total, last, state.previous_totals)?;
    let delta = match resolved {
        Delta::Baseline { next_totals } => {
            state.previous_totals = Some(next_totals);
            return None;
        }
        Delta::Spent { delta, next_totals } => {
            // Cached input is a SUBSET of input, so a reading that claims more
            // cache than input is clamped rather than trusted — otherwise the
            // uncached remainder goes negative and the price with it.
            let delta = RawUsage {
                cached_input_tokens: delta.cached_input_tokens.min(delta.input_tokens),
                ..delta
            };
            if delta.is_empty() {
                return None;
            }
            state.previous_totals = next_totals;
            delta
        }
    };

    let model = read_model(payload).or_else(|| state.current_model.clone());
    Some(Event {
        session_id: state.session_id.clone(),
        timestamp: timestamp.to_string(),
        event_key: event_key(timestamp, total, last),
        // Codex reports tokens and no dollars; the price table below turns a
        // rolled-up row into money at report time.
        estimated_cost_usd: None,
        model,
        cwd: state
            .current_cwd
            .clone()
            .or_else(|| state.session_cwd.clone()),
        input_tokens: delta.input_tokens,
        cached_input_tokens: delta.cached_input_tokens,
        output_tokens: delta.output_tokens,
        reasoning_output_tokens: delta.reasoning_output_tokens,
        total_tokens: delta.total_tokens,
    })
}

/// Reads a whole rollout, dropping records another file already claimed.
///
/// `claimed` is shared across the scan: a fork copies earlier records into a
/// new rollout, and counting the copy would bill the same work twice.
#[must_use]
pub fn read_rollout(
    text: &str,
    fallback_session_id: &str,
    claimed: &mut HashSet<String>,
) -> Vec<Event> {
    let mut state = ReadState {
        session_id: fallback_session_id.to_string(),
        ..ReadState::default()
    };
    let mut events = Vec::new();
    for line in text.lines() {
        // Every line updates the reader even when it is not spend — the state
        // is what the NEXT increment will be attributed to.
        if let Some(event) = read_record(line, &mut state)
            && claimed.insert(event.event_key.clone())
        {
            events.push(event);
        }
    }
    events
}

/// Dollars per million tokens, with the tier some models bill above a
/// context threshold.
#[derive(Clone, Copy)]
struct Pricing {
    input: f64,
    cached_input: f64,
    output: f64,
    /// `None` for a model that bills one flat rate at every context length.
    long_context: Option<(f64, f64, f64)>,
}

/// Where the long-context tier starts for the models that have one.
const LONG_CONTEXT_THRESHOLD: f64 = 272_000.0;

/// Orca's `MODEL_PRICING` for Codex.
fn pricing_of(model: &str) -> Option<Pricing> {
    let flat = |input, cached_input, output| {
        Some(Pricing {
            input,
            cached_input,
            output,
            long_context: None,
        })
    };
    let tiered = |input, cached_input, output, above: (f64, f64, f64)| {
        Some(Pricing {
            input,
            cached_input,
            output,
            long_context: Some(above),
        })
    };
    match model {
        "gpt-5" | "gpt-5.1" | "gpt-5.1-codex" | "gpt-5.1-codex-max" => flat(1.25, 0.125, 10.0),
        "gpt-5.2" | "gpt-5.2-codex" | "gpt-5.3" | "gpt-5.3-codex" | "gpt-5.3-codex-spark" => {
            flat(1.75, 0.175, 14.0)
        }
        "gpt-5.4-mini" => flat(0.75, 0.075, 4.5),
        "gpt-5.4-nano" => flat(0.2, 0.02, 1.25),
        "gpt-5.4-pro" | "gpt-5.5-pro" => tiered(30.0, 30.0, 180.0, (60.0, 60.0, 270.0)),
        "gpt-5.4" | "gpt-5.6-terra" => tiered(2.5, 0.25, 15.0, (5.0, 0.5, 22.5)),
        "gpt-5.5" | "gpt-5.6-sol" => tiered(5.0, 0.5, 30.0, (10.0, 1.0, 45.0)),
        "gpt-5.6-luna" => tiered(1.0, 0.1, 6.0, (2.0, 0.2, 9.0)),
        _ => None,
    }
}

/// The reasoning-effort words a model id can be suffixed with.
const REASONING_TIERS: [&str; 7] = ["minimal", "low", "medium", "high", "xhigh", "auto", "none"];

/// Strips a trailing `(high)`, or refuses the id when the parentheses hold
/// something else — a name this table has never met must not be priced as its
/// prefix.
///
/// The result is TRIMMED, which the original does not do, and the difference
/// is a real misprice rather than tidiness. `gpt-5.4-pro (high)` becomes
/// `gpt-5.4-pro ` there — with the space the name matches neither
/// `gpt-5.4-pro` nor `gpt-5.4-pro-`, so it falls through to the `gpt-5.4-`
/// prefix and is billed at $2.50/M instead of $30/M. The same slip makes
/// `gpt-5.6-luna (high)` match nothing at all and lose its price entirely.
/// It stays invisible upstream because the families where it fires most often
/// — `5.1`, `5.2`, `5.3` and their `-codex` variants — happen to be priced
/// identically.
fn strip_parenthesized_tier(model: &str) -> Option<String> {
    let Some(open) = model.rfind('(') else {
        return Some(model.to_string());
    };
    if !model.ends_with(')') {
        return Some(model.to_string());
    }
    let inside = &model[open + 1..model.len() - 1];
    if inside.contains('(') || inside.contains(')') {
        return Some(model.to_string());
    }
    if !REASONING_TIERS.contains(&inside.trim()) {
        return None;
    }
    Some(model[..open].trim_end().to_string())
}

/// Strips trailing `-high`, `-medium`, … up to four deep.
fn strip_dash_tiers(model: &str) -> String {
    let mut current = model.to_string();
    for _ in 0..4 {
        let Some(tier) = REASONING_TIERS
            .iter()
            .find(|tier| current.ends_with(&format!("-{tier}")))
        else {
            return current;
        };
        current.truncate(current.len() - tier.len() - 1);
    }
    current
}

/// The price-table key a rollout's model id maps to, or `None` when this table
/// has never heard of it.
#[must_use]
pub fn pricing_key(model: Option<&str>) -> Option<String> {
    let lowered = model?.to_lowercase();
    let lower = strip_dash_tiers(&strip_parenthesized_tier(lowered.trim())?);
    if lower == "gpt-5" || lower == "gpt-5-codex" {
        return Some("gpt-5".to_string());
    }
    // Longest name first: `gpt-5.1-codex-max` must not be answered by the
    // `gpt-5.1-codex` rule, nor `gpt-5.4-pro` by the bare `gpt-5.4` one.
    for family in [
        "gpt-5.1-codex-max",
        "gpt-5.1-codex",
        "gpt-5.1",
        "gpt-5.2-codex",
        "gpt-5.2",
        "gpt-5.3-codex-spark",
        "gpt-5.3-codex",
        "gpt-5.3",
        "gpt-5.4-mini",
        "gpt-5.4-nano",
        "gpt-5.4-pro",
        "gpt-5.4",
        "gpt-5.5-pro",
        "gpt-5.5",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
    ] {
        if lower == family || lower.starts_with(&format!("{family}-")) {
            return Some(family.to_string());
        }
    }
    // The bare `gpt-5.6` alias routes to Sol. Matched EXACTLY: a prefix rule
    // would swallow the tier ids above and any future cheaper variant.
    if lower == "gpt-5.6" {
        return Some("gpt-5.6-sol".to_string());
    }
    None
}

/// Splits a token count across the tier boundary and prices each part.
fn tiered_cost(tokens: f64, base: f64, above: Option<f64>) -> f64 {
    let Some(above_price) = above else {
        return tokens * base;
    };
    let below = tokens.min(LONG_CONTEXT_THRESHOLD);
    let over = (tokens - LONG_CONTEXT_THRESHOLD).max(0.0);
    below * base + over * above_price
}

/// What these tokens would have cost on the API, or `None` for a model with
/// no price on file.
///
/// Cached input is charged ONCE, at the cache rate, and the full-price input
/// is the remainder — the cached count is a subset of the input count, not a
/// separate bucket beside it. Reasoning output is not charged at all: it is
/// already inside the output figure.
#[must_use]
pub fn estimate_cost_usd(
    model: Option<&str>,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
) -> Option<f64> {
    let pricing = pricing_of(&pricing_key(model)?)?;
    let cached = cached_input_tokens.min(input_tokens).max(0);
    let uncached = (input_tokens - cached).max(0);
    let total = tiered_cost(
        uncached as f64,
        pricing.input,
        pricing.long_context.map(|(input, _, _)| input),
    ) + tiered_cost(
        cached as f64,
        pricing.cached_input,
        pricing.long_context.map(|(_, cached, _)| cached),
    ) + tiered_cost(
        output_tokens as f64,
        pricing.output,
        pricing.long_context.map(|(_, _, output)| output),
    );
    Some(total / 1_000_000.0)
}

/* ---- what this pane draws ------------------------------------------------
 *
 * The five-counter shapes live in [`crate::usage_report`] and the rollup that
 * fills them in [`crate::usage_ledger`], because OpenCode reports the same
 * five figures and the pane that draws one draws the other. They are
 * re-exported here so this module still reads as the Codex ledger's whole
 * answer. */
pub use crate::usage_ledger::{
    DailyAggregate, Ledger, LocationBreakdown, Session, aggregate, finalize, merge,
};
pub use crate::usage_report::{BreakdownRow, DailyPoint, Report, SessionRow, Summary};

/// Builds every figure the Codex pane draws.
///
/// Codex reports no dollars, so every row is priced here from
/// [`estimate_cost_usd`] — the difference between this ledger and the other
/// one that shares the rollup, and the only reason this wrapper exists.
#[must_use]
pub fn report(
    ledger: &Ledger,
    scope: Scope,
    range: Range,
    now_ms: i64,
    offset_minutes: i32,
) -> Report {
    crate::usage_ledger::report(
        ledger,
        scope,
        range,
        now_ms,
        offset_minutes,
        crate::usage_ledger::Dollars::FromTable(estimate_cost_usd),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const KST: i32 = 9 * 60;

    fn usage(input: i64, cached: i64, output: i64, reasoning: i64, total: i64) -> RawUsage {
        RawUsage {
            input_tokens: input,
            cached_input_tokens: cached,
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            total_tokens: total,
        }
    }

    fn token_line(timestamp: &str, total: &str, last: &str) -> String {
        format!(
            r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{total},"last_token_usage":{last}}}}}}}"#
        )
    }

    fn counts(input: i64, cached: i64, output: i64, reasoning: i64, total: i64) -> String {
        format!(
            r#"{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":{reasoning},"total_tokens":{total}}}"#
        )
    }

    /// The running total is a BASELINE, and `last_token_usage` is the spend.
    ///
    /// This is the whole difference from Claude's format. A rollout's
    /// `total_token_usage` grows across the conversation, so adding those
    /// numbers charges a long session once for every event it ever wrote —
    /// and the result is merely large rather than absurd, which is why it
    /// survives a glance.
    #[test]
    fn the_running_total_is_a_baseline_and_the_last_reading_is_the_spend() {
        let mut state = ReadState::default();
        let first = read_record(
            &token_line(
                "2026-08-19T01:00:00Z",
                &counts(100, 0, 20, 5, 120),
                &counts(100, 0, 20, 5, 120),
            ),
            &mut state,
        )
        .expect("the first reading is spend");
        assert_eq!(first.input_tokens, 100);
        assert_eq!(first.output_tokens, 20);

        // The total has climbed to 300/60, but only 200/40 was spent since.
        let second = read_record(
            &token_line(
                "2026-08-19T01:05:00Z",
                &counts(300, 0, 60, 15, 360),
                &counts(200, 0, 40, 10, 240),
            ),
            &mut state,
        )
        .expect("the second reading is spend");
        assert_eq!(
            second.input_tokens, 200,
            "the running total was billed instead of the increment"
        );
        assert_eq!(second.output_tokens, 40);
    }

    /// A total with no `last` is differenced against the previous total.
    #[test]
    fn a_total_without_an_increment_is_differenced() {
        let mut state = ReadState::default();
        assert_eq!(
            resolve_delta(Some(usage(100, 0, 20, 0, 120)), None, None),
            Some(Delta::Spent {
                delta: usage(100, 0, 20, 0, 120),
                next_totals: Some(usage(100, 0, 20, 0, 120)),
            }),
            "the first total is spend in full"
        );
        state.previous_totals = Some(usage(100, 0, 20, 0, 120));
        let grown = resolve_delta(Some(usage(250, 0, 45, 0, 295)), None, state.previous_totals);
        let Some(Delta::Spent { delta, .. }) = grown else {
            panic!("a grown total is spend");
        };
        assert_eq!(delta.input_tokens, 150);
        assert_eq!(delta.output_tokens, 25);
    }

    /// A total that RESTARTS is a new baseline, never a spend.
    ///
    /// Compaction and resume reset the counters. Reading that reset as spend
    /// would invent a whole session's worth of tokens out of a session that
    /// had just been made smaller.
    #[test]
    fn a_restarted_total_is_a_baseline_rather_than_a_spend() {
        let previous = usage(500_000, 0, 90_000, 0, 590_000);
        // Far below the previous total: a genuine compaction.
        let after = resolve_delta(Some(usage(4_000, 0, 800, 0, 4_800)), None, Some(previous));
        assert_eq!(
            after,
            Some(Delta::Baseline {
                next_totals: usage(4_000, 0, 800, 0, 4_800)
            }),
            "a compaction was billed as if it were fresh spend"
        );
        // The identical reading twice is not spend at all.
        assert_eq!(
            resolve_delta(Some(previous), None, Some(previous)),
            None,
            "a repeated total was counted a second time"
        );
    }

    /// A regression that is nearly the previous total is a stale duplicate.
    #[test]
    fn a_nearly_equal_regression_is_dropped_as_stale() {
        let previous = usage(100_000, 0, 10_000, 0, 110_000);
        let last = usage(500, 0, 50, 0, 550);
        // 99.5% of the previous magnitude — a stale copy, not a real restart.
        let stale = resolve_delta(
            Some(usage(99_500, 0, 9_950, 0, 109_450)),
            Some(last),
            Some(previous),
        );
        assert_eq!(stale, None, "a stale duplicate was billed again");
        // A true restart, with a `last` beside it, still yields the increment.
        let restarted = resolve_delta(Some(usage(600, 0, 60, 0, 660)), Some(last), Some(previous));
        assert!(
            matches!(restarted, Some(Delta::Spent { delta, .. }) if delta == last),
            "a real restart lost its increment: {restarted:?}"
        );
    }

    /// A `token_count` with no `info` is a rate-limit ping, not usage.
    #[test]
    fn a_pingless_token_count_is_not_a_scan_error() {
        let mut state = ReadState::default();
        let ping = r#"{"timestamp":"2026-08-19T01:00:00Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#;
        assert!(read_record(ping, &mut state).is_none());
        assert!(
            state.previous_totals.is_none(),
            "a rate-limit ping moved the baseline"
        );
    }

    /// The session header and the turn headers place the spend that follows.
    #[test]
    fn the_headers_place_the_spend_that_follows_them() {
        let mut state = ReadState::default();
        let meta = r#"{"timestamp":"2026-08-19T00:59:00Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/w/repo"}}"#;
        assert!(
            read_record(meta, &mut state).is_none(),
            "a header is not spend"
        );
        assert_eq!(state.session_id, "sess-1");

        let turn = r#"{"timestamp":"2026-08-19T00:59:30Z","type":"turn_context","payload":{"cwd":"/w/repo/sub","model":"gpt-5.3-codex"}}"#;
        assert!(read_record(turn, &mut state).is_none());

        let event = read_record(
            &token_line(
                "2026-08-19T01:00:00Z",
                &counts(10, 0, 2, 0, 12),
                &counts(10, 0, 2, 0, 12),
            ),
            &mut state,
        )
        .expect("spend");
        assert_eq!(event.session_id, "sess-1");
        assert_eq!(
            event.cwd.as_deref(),
            Some("/w/repo/sub"),
            "the turn's directory did not reach the spend that followed it"
        );
        assert_eq!(event.model.as_deref(), Some("gpt-5.3-codex"));
    }

    /// A forked rollout copies records; the copy is counted once.
    #[test]
    fn a_forked_rollout_does_not_bill_the_copied_records_twice() {
        let lines = [
            r#"{"timestamp":"2026-08-19T00:59:00Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/w/repo"}}"#.to_string(),
            token_line("2026-08-19T01:00:00Z", &counts(100, 0, 20, 0, 120), &counts(100, 0, 20, 0, 120)),
        ];
        let original = lines.join("\n");
        // The fork rewrites the session id but keeps the record byte for byte.
        let forked = original.replace("sess-1", "sess-2");

        let mut claimed = HashSet::new();
        let first = read_rollout(&original, "file-a", &mut claimed);
        let second = read_rollout(&forked, "file-b", &mut claimed);
        assert_eq!(first.len(), 1);
        assert!(
            second.is_empty(),
            "the fork's copy of an already-counted record was billed again"
        );
    }

    /// Cached input is a subset of input, and is charged once.
    #[test]
    fn cached_input_is_billed_at_the_cache_rate_and_never_twice() {
        // gpt-5.3-codex: input 1.75, cached 0.175, output 14 per million.
        // 1M input of which 400k cached → 600k at 1.75 + 400k at 0.175.
        let cost = estimate_cost_usd(Some("gpt-5.3-codex"), 1_000_000, 400_000, 0).expect("priced");
        let expected = (600_000.0 * 1.75 + 400_000.0 * 0.175) / 1_000_000.0;
        assert!((cost - expected).abs() < 1e-12, "{cost} vs {expected}");

        // A reading claiming more cache than input is clamped, not trusted.
        let clamped = estimate_cost_usd(Some("gpt-5.3-codex"), 1000, 5000, 0).expect("priced");
        assert!(
            (clamped - 1000.0 * 0.175 / 1_000_000.0).abs() < 1e-12,
            "an impossible cache count drove the uncached input negative"
        );

        // Reasoning is inside output and costs nothing extra.
        let with_output =
            estimate_cost_usd(Some("gpt-5.3-codex"), 0, 0, 1_000_000).expect("priced");
        assert!((with_output - 14.0).abs() < 1e-12);
    }

    /// The model id resolves through its reasoning suffix to a price.
    #[test]
    fn a_model_is_priced_through_its_reasoning_suffix() {
        assert_eq!(
            pricing_key(Some("gpt-5.3-codex")).as_deref(),
            Some("gpt-5.3-codex")
        );
        assert_eq!(
            pricing_key(Some("gpt-5.3-codex-high")).as_deref(),
            Some("gpt-5.3-codex")
        );
        assert_eq!(
            pricing_key(Some("GPT-5.3-Codex (xhigh)")).as_deref(),
            Some("gpt-5.3-codex")
        );
        assert_eq!(pricing_key(Some("gpt-5-codex")).as_deref(), Some("gpt-5"));
        // The longer family name wins over the shorter one that prefixes it.
        assert_eq!(
            pricing_key(Some("gpt-5.1-codex-max")).as_deref(),
            Some("gpt-5.1-codex-max"),
            "a max variant was priced as the plain codex model"
        );
        assert_eq!(
            pricing_key(Some("gpt-5.4-pro")).as_deref(),
            Some("gpt-5.4-pro")
        );
        // The bare alias routes to Sol, but only when it is exactly that.
        assert_eq!(pricing_key(Some("gpt-5.6")).as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            pricing_key(Some("gpt-5.6-luna")).as_deref(),
            Some("gpt-5.6-luna")
        );
        // The trim the original lacks: without it these fall through to a
        // cheaper family, or off the table entirely.
        assert_eq!(
            pricing_key(Some("gpt-5.4-pro (high)")).as_deref(),
            Some("gpt-5.4-pro"),
            "a pro model was billed as the standard one — a twelvefold underprice"
        );
        assert_eq!(
            pricing_key(Some("gpt-5.6-luna (low)")).as_deref(),
            Some("gpt-5.6-luna")
        );
        // Parentheses holding something else is a name this table has not met.
        assert_eq!(pricing_key(Some("gpt-5.3-codex (experimental)")), None);
        assert_eq!(pricing_key(Some("some-other-model")), None);
        assert_eq!(estimate_cost_usd(None, 1_000, 0, 1_000), None);
    }

    /// The long-context tier bills the excess higher.
    #[test]
    fn the_long_context_tier_prices_the_excess() {
        // gpt-5.4: input 2.5 below 272k, 5 above.
        let cost = estimate_cost_usd(Some("gpt-5.4"), 372_000, 0, 0).expect("priced");
        let expected = (272_000.0 * 2.5 + 100_000.0 * 5.0) / 1_000_000.0;
        assert!((cost - expected).abs() < 1e-9, "{cost} vs {expected}");
        // gpt-5.3 bills flat at every length.
        let flat = estimate_cost_usd(Some("gpt-5.3"), 372_000, 0, 0).expect("priced");
        assert!((flat - 372_000.0 * 1.75 / 1_000_000.0).abs() < 1e-9);
    }

    /// The pane's dollars come from this module's table, through the wrapper.
    #[test]
    fn the_wrapper_prices_the_rolled_up_rows() {
        let event = Event {
            session_id: "sess".to_string(),
            timestamp: "2026-08-19T01:00:00Z".to_string(),
            event_key: "one".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            cwd: Some("/w/repo".to_string()),
            estimated_cost_usd: None,
            input_tokens: 1_000,
            cached_input_tokens: 400,
            output_tokens: 200,
            reasoning_output_tokens: 50,
            total_tokens: 1_200,
        };
        let ledger = aggregate(vec![event], &[], KST);
        let now = crate::civil::epoch_ms_of_iso("2026-08-19T02:00:00Z").expect("stamp");
        let answer = report(&ledger, Scope::All, Range::All, now, KST);
        let expected = estimate_cost_usd(Some("gpt-5.3-codex"), 1_000, 400, 200).expect("priced");
        let priced = answer.summary.estimated_cost_usd.expect("a price");
        assert!(
            (priced - expected).abs() < 1e-12,
            "the report priced {priced} where the table says {expected}"
        );
        assert_eq!(
            answer.model_breakdown[0].estimated_cost_usd,
            Some(expected),
            "the model list lost the price the table gave it"
        );
        assert_eq!(
            answer.project_breakdown[0].estimated_cost_usd, None,
            "a project row was priced from a table that keys on models"
        );
    }
}
