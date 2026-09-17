//! The token ledger for OpenCode, whose turns live in a SQLite file.
//!
//! [`crate::usage_stats_codex`] reads a rollout of JSON lines; this one reads
//! rows a database handed over. What both produce is a
//! [`crate::usage_ledger::Entry`], so everything after the read — sessions,
//! days, scope, range, totals — is the shared rollup and is not written twice.
//!
//! The SQL is NOT here. Finding the databases and running the queries is I/O
//! and lives in the shell; what lives here is the rule that turns one row into
//! a turn, which is the part a person could disagree with and the part worth a
//! test.
//!
//! ## What is measured, and against what
//!
//! Read out of Orca's `src/main/opencode-usage/*` (1.4.184) —
//! `opencode-usage-row-queries.ts`, `opencode-usage-row-parsing.ts`. The
//! DATABASE shape was verified against a real `opencode.db` on this machine:
//! 86 sessions with usage, `session_message` empty while `message` held 6067
//! rows, so a reader that knows only one of those table names reports zero and
//! calls it "OpenCode not used".
//!
//! ## Two deviations from the original, both deliberate
//!
//! Every one of the 5224 assistant rows on this machine that carries a total
//! satisfies `total = input + output + reasoning + cache.read` — 5224 of 5224,
//! not a majority. So for THIS vendor the cached count is its own counter
//! beside the input rather than a share of it, and the vendor's own arithmetic
//! says so.
//!
//! 1. The original clamps the cached count with `min(cache.read, input)`. On
//!    the machine above that turns 580,555,520 cached tokens into 18,693,737 —
//!    it throws away 96.8% of the largest counter in the ledger. We read it as
//!    reported.
//! 2. The original synthesizes a missing total as `input + output +
//!    reasoning`, which contradicts the vendor's own total by leaving the
//!    cache out. We add all four.
//!
//! Neither changes what a turn cost, because OpenCode reports its own dollars
//! and this module never prices anything.
//!
//! A third deviation is in [`richer_shape`], where the same machine's numbers
//! say which of the database's two shapes to read.

use serde_json::Value;

use crate::civil::iso_utc_of;
use crate::usage_ledger::{Dollars, Entry, Ledger, Report};
use crate::usage_stats::{Range, Scope};

/// One row of the per-message query, as the database handed it over.
///
/// `data` is the message's own JSON blob, which is where OpenCode keeps the
/// counters. The columns beside it are the session's, joined in, because a
/// message that never named a directory still happened somewhere.
#[derive(Clone, Debug, Default)]
pub struct MessageRow {
    pub session_id: String,
    pub time_created: i64,
    pub time_updated: Option<i64>,
    pub data: String,
    pub directory: Option<String>,
    pub worktree: Option<String>,
    /// `session.model`, which is a JSON object in the generations that have it.
    pub session_model: Option<String>,
}

/// One row of the per-session query, for the newer databases that keep their
/// own totals.
///
/// Reading one row per session is the difference between 86 rows and 6067
/// blobs to parse, so it is tried first. The counters are the session's whole
/// life, so the turn it produces is that session as a single turn — which is
/// what the pane shows anyway, since it draws sessions and days.
#[derive(Clone, Debug, Default)]
pub struct SessionRow {
    pub session_id: String,
    pub time_created: i64,
    pub time_updated: Option<i64>,
    pub directory: Option<String>,
    pub worktree: Option<String>,
    pub session_model: Option<String>,
    pub cost: f64,
    pub tokens_input: i64,
    pub tokens_output: i64,
    pub tokens_reasoning: i64,
    pub tokens_cache_read: i64,
}

/// The counters a turn carries, whichever row they came from.
struct Counters {
    input: i64,
    cached_input: i64,
    output: i64,
    reasoning: i64,
    total: i64,
}

impl Counters {
    /// The vendor's total when it reported one, and all four counters added up
    /// when it did not — see the module note on why the cache is in that sum.
    fn with_total(input: i64, cached_input: i64, output: i64, reasoning: i64, total: i64) -> Self {
        Self {
            input,
            cached_input,
            output,
            reasoning,
            total: if total > 0 {
                total
            } else {
                input + cached_input + output + reasoning
            },
        }
    }

    /// A row whose every counter is empty is not a turn.
    fn is_empty(&self) -> bool {
        self.input + self.cached_input + self.output + self.reasoning + self.total <= 0
    }
}

/// Turns one message row into a turn, or `None` when it is not one.
#[must_use]
pub fn entry_of_message(row: &MessageRow) -> Option<Entry> {
    let data: Value = serde_json::from_str(&row.data).ok()?;
    let data = data.as_object()?;
    let tokens = object_of(data.get("tokens"))?;
    let cache = object_of(tokens.get("cache"));
    let counters = Counters::with_total(
        whole(tokens.get("input")),
        whole(cache.as_ref().and_then(|held| held.get("read"))),
        whole(tokens.get("output")),
        whole(tokens.get("reasoning")),
        whole(tokens.get("total")),
    );
    if counters.is_empty() {
        return None;
    }
    let time = object_of(data.get("time"));
    let stamp = millis(time.as_ref().and_then(|held| held.get("completed")))
        .or_else(|| millis(time.as_ref().and_then(|held| held.get("created"))))
        .or_else(|| row.time_updated.and_then(from_epoch))
        .or_else(|| from_epoch(row.time_created))?;
    let cost = money(data.get("cost"));
    Some(entry(
        &row.session_id,
        stamp,
        model_of(data, row.session_model.as_deref()),
        object_of(data.get("path"))
            .and_then(|path| text(path.get("cwd")))
            .or_else(|| row.directory.clone())
            .or_else(|| row.worktree.clone()),
        cost,
        counters,
    ))
}

/// Turns one session's own totals into a turn, or `None` when it spent
/// nothing.
#[must_use]
pub fn entry_of_session(row: &SessionRow) -> Option<Entry> {
    let counters = Counters::with_total(
        row.tokens_input,
        row.tokens_cache_read,
        row.tokens_output,
        row.tokens_reasoning,
        0,
    );
    if counters.is_empty() {
        return None;
    }
    let stamp = row
        .time_updated
        .and_then(from_epoch)
        .or_else(|| from_epoch(row.time_created))?;
    Some(entry(
        &row.session_id,
        stamp,
        row.session_model.as_deref().and_then(named_model),
        row.directory.clone().or_else(|| row.worktree.clone()),
        (row.cost > 0.0).then_some(row.cost),
        counters,
    ))
}

fn entry(
    session_id: &str,
    stamp: i64,
    model: Option<String>,
    cwd: Option<String>,
    estimated_cost_usd: Option<f64>,
    counters: Counters,
) -> Entry {
    Entry {
        session_id: session_id.to_string(),
        timestamp: iso_utc_of(stamp),
        model,
        cwd,
        input_tokens: counters.input,
        cached_input_tokens: counters.cached_input,
        output_tokens: counters.output,
        reasoning_output_tokens: counters.reasoning,
        total_tokens: counters.total,
        estimated_cost_usd,
        // Every row this reader sees is a distinct primary key, so there is
        // nothing to claim — unlike a rollout, which a fork copies wholesale.
        event_key: String::new(),
    }
}

/// Which of a database's two shapes holds the whole story.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// One turn per assistant message: real days, real models, real counts.
    Messages,
    /// One turn per session, from the totals the newer databases keep.
    SessionTotals,
}

/// Reads the message rows when they account for everything the session totals
/// do, and the session totals when they do not.
///
/// The original prefers the session totals whenever they exist, because
/// parsing every message blob in JavaScript is slow. Measured on this machine
/// the two agree exactly — 35,175,005 input, 1,731,586 output, 357,449
/// reasoning, 580,555,520 cached across the same 86 sessions — but the message
/// rows also carry the DAY and the MODEL of each turn, which one row per
/// session cannot: a session that ran over three days lands entirely on the
/// last of them and the chart draws a spike that never happened.
///
/// So the richer shape wins whenever it is complete. A database whose messages
/// were pruned while its session totals were kept reports less from the rows
/// than from the totals, and there the totals are the honest answer even
/// though they are coarse.
#[must_use]
pub fn richer_shape(message_tokens: i64, session_tokens: i64) -> Shape {
    if message_tokens > 0 && message_tokens >= session_tokens {
        Shape::Messages
    } else {
        Shape::SessionTotals
    }
}

/// Builds every figure the OpenCode pane draws.
///
/// OpenCode reports its own dollars per turn, so nothing here prices anything:
/// the rollup carried the figure and the report adds it up.
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
        Dollars::AsReported,
    )
}

/// The model, named where a row is known to name one.
///
/// A message names its own; a session names one for every message it holds.
/// Either can be a bare id beside a provider, or a JSON object holding both.
fn model_of(data: &serde_json::Map<String, Value>, session_model: Option<&str>) -> Option<String> {
    let direct = text(data.get("modelID")).or_else(|| text(data.get("modelId")));
    if let Some(model) = direct {
        let provider = text(data.get("providerID")).or_else(|| text(data.get("providerId")));
        return Some(qualified(provider, model));
    }
    object_of(data.get("model"))
        .and_then(|held| from_model_object(&held))
        .or_else(|| session_model.and_then(named_model))
}

/// The model a `session.model` column holds — an object in the generations
/// that have the column at all.
///
/// Public because the vault panel names the same column
/// ([`crate::vault_opencode`]) and one model naming rule for the whole app is
/// the point: a card saying `gpt-5.5-fast` beside a usage row saying
/// `openai/gpt-5.5-fast` is one model wearing two names.
#[must_use]
pub fn named_model(session_model: &str) -> Option<String> {
    let value: Value = serde_json::from_str(session_model).ok()?;
    from_model_object(value.as_object()?)
}

fn from_model_object(held: &serde_json::Map<String, Value>) -> Option<String> {
    let model = text(held.get("modelID")).or_else(|| text(held.get("id")))?;
    Some(qualified(text(held.get("providerID")), model))
}

fn qualified(provider: Option<String>, model: String) -> String {
    match provider {
        Some(provider) => format!("{provider}/{model}"),
        None => model,
    }
}

/// A JSON object, whether it arrived as one or as a string holding one.
///
/// OpenCode writes both shapes for the same field across its generations, and
/// a reader that knows only the nested one drops every counter in the older
/// files.
fn object_of(value: Option<&Value>) -> Option<serde_json::Map<String, Value>> {
    match value? {
        Value::Object(held) => Some(held.clone()),
        Value::String(text) => match serde_json::from_str::<Value>(text).ok()? {
            Value::Object(held) => Some(held),
            _ => None,
        },
        _ => None,
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A counter, however JSON spelled it. A negative one is not a count.
fn whole(value: Option<&Value>) -> i64 {
    let read = match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::String(text)) => text.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };
    if read.is_finite() && read > 0.0 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a token count above i64 is a corrupt row, and saturates rather than wraps"
        )]
        {
            read.round() as i64
        }
    } else {
        0
    }
}

/// Dollars, when the vendor reported any. Zero is not a report.
fn money(value: Option<&Value>) -> Option<f64> {
    let read = match value {
        Some(Value::Number(number)) => number.as_f64()?,
        Some(Value::String(text)) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (read.is_finite() && read > 0.0).then_some(read)
}

/// A stamp from a JSON field, in whichever unit it was written in.
fn millis(value: Option<&Value>) -> Option<i64> {
    from_epoch(whole(value))
}

/// Seconds and milliseconds both appear in these columns, so a stamp small
/// enough to be a second count is read as one. The boundary is the original's:
/// 10^10 milliseconds is 1970 and 10^10 seconds is the year 2286, so no real
/// stamp is ambiguous.
fn from_epoch(stamp: i64) -> Option<i64> {
    if stamp <= 0 {
        return None;
    }
    Some(if stamp < 10_000_000_000 {
        stamp * 1_000
    } else {
        stamp
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real row, copied out of `~/.local/share/opencode/opencode.db` on this
    /// machine — model, provider, stamps and counters exactly as found.
    const REAL_BLOB: &str = r#"{"role":"assistant","modelID":"gpt-5.5-fast","providerID":"openai","cost":0,
        "tokens":{"total":12076,"input":6416,"output":28,"reasoning":0,"cache":{"write":0,"read":5632}},
        "time":{"created":1777449250360,"completed":1777449252541},
        "path":{"cwd":"/Users/dev/2026/pc","root":"/Users/dev/2026/pc"}}"#;

    fn message(data: &str) -> MessageRow {
        MessageRow {
            session_id: "ses_1".to_string(),
            time_created: 1_777_449_250,
            time_updated: None,
            data: data.to_string(),
            directory: Some("/fallback/dir".to_string()),
            worktree: Some("/fallback/worktree".to_string()),
            session_model: None,
        }
    }

    /// The row above, read as the vendor wrote it.
    #[test]
    fn a_real_row_reads_as_the_vendor_wrote_it() {
        let turn = entry_of_message(&message(REAL_BLOB)).expect("a turn");
        assert_eq!(turn.session_id, "ses_1");
        assert_eq!(turn.model.as_deref(), Some("openai/gpt-5.5-fast"));
        assert_eq!(turn.cwd.as_deref(), Some("/Users/dev/2026/pc"));
        // `completed`, not `created` — the turn is spent when it finishes.
        assert_eq!(turn.timestamp, "2026-04-29T07:54:12.541Z");
        assert_eq!(turn.input_tokens, 6_416);
        assert_eq!(turn.cached_input_tokens, 5_632);
        assert_eq!(turn.output_tokens, 28);
        assert_eq!(turn.reasoning_output_tokens, 0);
        assert_eq!(turn.total_tokens, 12_076);
        assert_eq!(
            turn.total_tokens,
            turn.input_tokens
                + turn.output_tokens
                + turn.reasoning_output_tokens
                + turn.cached_input_tokens,
            "this vendor's total IS the four counters; the fallback must match it"
        );
        assert_eq!(
            turn.estimated_cost_usd, None,
            "a reported zero is no report, not a free turn"
        );
    }

    /// The cached count is its own counter, so nothing clips it to the input.
    ///
    /// The original's `min(cache.read, input)` loses 96.8% of this ledger's
    /// largest counter on the machine this was read from.
    #[test]
    fn the_cached_count_is_not_clipped_to_the_input() {
        let turn = entry_of_message(&message(
            r#"{"tokens":{"input":100,"output":10,"reasoning":0,"cache":{"read":90000}},
                "time":{"completed":1777449252541}}"#,
        ))
        .expect("a turn");
        assert_eq!(turn.cached_input_tokens, 90_000);
        assert_eq!(
            turn.total_tokens, 90_110,
            "a missing total left the cache out of the sum"
        );
    }

    /// A row with no counters, or none at all, is not a turn.
    #[test]
    fn a_row_that_spent_nothing_is_not_a_turn() {
        assert!(entry_of_message(&message("not json")).is_none());
        assert!(entry_of_message(&message(r#"{"role":"user"}"#)).is_none());
        assert!(
            entry_of_message(&message(
                r#"{"tokens":{"input":0,"output":0},"time":{"completed":1777449252541}}"#
            ))
            .is_none()
        );
    }

    /// The stamp falls back through the fields, in the original's order, and a
    /// second count is read as seconds.
    #[test]
    fn the_stamp_falls_back_through_the_columns() {
        let counters = r#""tokens":{"input":10,"output":1}"#;
        let created_only = entry_of_message(&message(&format!(
            r#"{{{counters},"time":{{"created":1777449250360}}}}"#
        )))
        .expect("a turn");
        assert_eq!(created_only.timestamp, "2026-04-29T07:54:10.360Z");

        let mut row = message(&format!("{{{counters}}}"));
        row.time_updated = Some(1_777_449_260_000);
        let updated = entry_of_message(&row).expect("a turn");
        assert_eq!(updated.timestamp, "2026-04-29T07:54:20.000Z");

        row.time_updated = None;
        let created = entry_of_message(&row).expect("a turn");
        assert_eq!(
            created.timestamp, "2026-04-29T07:54:10.000Z",
            "a column in seconds was read as milliseconds"
        );

        row.time_created = 0;
        assert!(
            entry_of_message(&row).is_none(),
            "a turn with no moment was kept anyway"
        );
    }

    /// The directory falls back to the session's columns.
    #[test]
    fn the_place_falls_back_to_the_sessions_columns() {
        let mut row =
            message(r#"{"tokens":{"input":10,"output":1},"time":{"completed":1777449252541}}"#);
        assert_eq!(
            entry_of_message(&row).expect("a turn").cwd.as_deref(),
            Some("/fallback/dir")
        );
        row.directory = None;
        assert_eq!(
            entry_of_message(&row).expect("a turn").cwd.as_deref(),
            Some("/fallback/worktree")
        );
    }

    /// The model is read from wherever this vendor's generations put it.
    #[test]
    fn a_model_is_named_from_wherever_the_row_puts_it() {
        let named = |data: &str, session_model: Option<&str>| {
            let mut row = message(data);
            row.session_model = session_model.map(str::to_string);
            entry_of_message(&row).expect("a turn").model
        };
        let counters = r#""tokens":{"input":10,"output":1},"time":{"completed":1777449252541}"#;
        assert_eq!(
            named(&format!(r#"{{{counters},"modelId":"grok-5"}}"#), None).as_deref(),
            Some("grok-5"),
            "a provider-less id lost its model"
        );
        assert_eq!(
            named(
                &format!(r#"{{{counters},"model":{{"providerID":"anthropic","modelID":"claude-opus-5"}}}}"#),
                None
            )
            .as_deref(),
            Some("anthropic/claude-opus-5")
        );
        assert_eq!(
            named(
                &format!("{{{counters}}}"),
                Some(r#"{"providerID":"openai","id":"gpt-5.5"}"#)
            )
            .as_deref(),
            Some("openai/gpt-5.5"),
            "the session's own column was not consulted"
        );
        assert_eq!(named(&format!("{{{counters}}}"), Some("not json")), None);
    }

    /// The richer shape wins when it is complete, and loses when it is not.
    #[test]
    fn the_shape_is_the_one_that_accounts_for_everything() {
        // What this machine reports: the two agree, so the rows win.
        assert_eq!(richer_shape(617_819_560, 617_819_560), Shape::Messages);
        // Rows that saw more than the totals did are still the richer read.
        assert_eq!(richer_shape(700, 600), Shape::Messages);
        // Pruned messages under retained totals: the coarse answer is honest.
        assert_eq!(richer_shape(10, 600), Shape::SessionTotals);
        // Nothing in the rows at all — the older databases, and this machine's
        // empty `session_message` table.
        assert_eq!(richer_shape(0, 0), Shape::SessionTotals);
    }

    /// A session's own totals become one turn, with the dollars it reported.
    #[test]
    fn a_sessions_totals_become_one_turn() {
        let row = SessionRow {
            session_id: "ses_2".to_string(),
            time_created: 1_777_449_250,
            time_updated: Some(1_777_449_260_000),
            directory: Some("/w/repo".to_string()),
            worktree: None,
            session_model: Some(r#"{"providerID":"openai","modelID":"gpt-5.5"}"#.to_string()),
            cost: 1.25,
            tokens_input: 4_653,
            tokens_output: 215,
            tokens_reasoning: 60,
            tokens_cache_read: 13_824,
        };
        let turn = entry_of_session(&row).expect("a turn");
        assert_eq!(turn.model.as_deref(), Some("openai/gpt-5.5"));
        assert_eq!(turn.cwd.as_deref(), Some("/w/repo"));
        assert_eq!(turn.timestamp, "2026-04-29T07:54:20.000Z");
        assert_eq!(turn.cached_input_tokens, 13_824);
        // The figure the vendor itself reports for this session is 18752, and
        // the original's synthesized total would have said 4928.
        assert_eq!(turn.total_tokens, 18_752);
        assert_eq!(turn.estimated_cost_usd, Some(1.25));

        let empty = SessionRow {
            session_id: "ses_3".to_string(),
            ..Default::default()
        };
        assert!(entry_of_session(&empty).is_none());
    }
}
