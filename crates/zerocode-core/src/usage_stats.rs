//! Token analytics read out of the agents' own transcripts.
//!
//! Orca's Stats pane answers two different questions that look alike on
//! screen. The status bar's percentages are a PLAN LIMIT — how much of a
//! subscription window is spent — and they come from the vendor's own
//! endpoint ([`crate::usage_limit`] and the shell's `usage_oauth`). This
//! module answers the other one: what the tokens actually were, counted off
//! the transcripts Claude Code writes to disk, broken down by day, model,
//! project and session, with an API-equivalent price attached.
//!
//! The two never mix. A plan window can sit at 40% while the token ledger
//! shows millions, because a subscription is not billed per token; showing
//! either number under the other's label is the one mistake this pane can
//! make that a person cannot detect.
//!
//! ## What is measured, and against what
//!
//! The record shape, the dedupe rule, the aggregation keys, the scope and
//! range filters, and the price table are read out of Orca's own
//! `src/main/claude-usage/*` (1.4.184) — `transcript-record-parser.ts`,
//! `usage-aggregation.ts`, `claude-usage-report-aggregation.ts`,
//! `claude-usage-scope-filters.ts`, `claude-usage-session-rows.ts`,
//! `claude-model-pricing.ts`. The FILE FORMAT was verified against real
//! `~/.claude/projects/*/*.jsonl` on this machine, because a parser measured
//! only from somebody else's parser is a guess about a third party's format.
//!
//! ## Why this module is pure
//!
//! Nothing here opens a file, reads a clock, or asks what timezone the
//! machine is in. The walk belongs to the shell, `now` arrives as a
//! parameter, and the UTC offset arrives from the window — which is the only
//! layer that knows which midnight the person means. A day boundary is the
//! kind of thing that is wrong for half a year in a timezone nobody on the
//! team lives in, and an injected offset is how it stays testable.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::civil::{epoch_ms_of_iso, iso_date_of};

/// Which transcripts count toward the figures.
///
/// Orca's own pair, under this product's name: `zerocode` keeps only turns
/// whose working directory fell inside a worktree this app manages, `all`
/// keeps every Claude session on the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Zerocode,
    All,
}

impl Scope {
    /// The wire word, refusing anything else rather than guessing.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "zerocode" => Some(Self::Zerocode),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

/// How far back the figures reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Range {
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
    #[serde(rename = "90d")]
    Quarter,
    All,
}

impl Range {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "7d" => Some(Self::Week),
            "30d" => Some(Self::Month),
            "90d" => Some(Self::Quarter),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// Days this range spans, or `None` for all of history.
    #[must_use]
    pub const fn days(self) -> Option<i64> {
        match self {
            Self::Week => Some(7),
            Self::Month => Some(30),
            Self::Quarter => Some(90),
            Self::All => None,
        }
    }
}

/// The earliest local day this range keeps, `None` when it keeps everything.
///
/// Orca's `getUsageRangeCutoff`: the window is inclusive of today, so seven
/// days means today and the six before it — `days - 1`, not `days`.
#[must_use]
pub fn range_cutoff(range: Range, now_ms: i64, offset_minutes: i32) -> Option<String> {
    let days = range.days()?;
    Some(iso_date_of(
        now_ms - (days - 1) * 86_400_000,
        offset_minutes,
    ))
}

/// One assistant turn, as the transcript recorded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceTurn {
    pub session_id: String,
    pub timestamp: String,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// The strongest stable identity this row carried, used to collapse the
    /// repeats a stream writes. `None` means the row cannot be matched to any
    /// other and is therefore always kept.
    pub dedupe_key: Option<String>,
}

/// The identity a turn is collapsed by.
///
/// Orca's `buildClaudeUsageDedupeKey`, in its order: a fork rewrites
/// `sessionId` but keeps the message and request ids, so the pair is the
/// strongest claim; `uuid` is the fallback for older rows that carry no
/// `requestId`.
fn dedupe_key(
    message_id: Option<&str>,
    request_id: Option<&str>,
    uuid: Option<&str>,
) -> Option<String> {
    let trimmed = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let message_id = trimmed(message_id);
    let request_id = trimmed(request_id);
    match (message_id, request_id) {
        (Some(message), Some(request)) => Some(format!("{message}:{request}")),
        (Some(message), None) => Some(format!("msg:{message}")),
        (None, _) => trimmed(uuid).map(|uuid| format!("uuid:{uuid}")),
    }
}

/// One JSONL line, or `None` when it is not a billable assistant turn.
///
/// `fallback_session_id` is the file's own stem, which is what Claude Code
/// names a session file: rows written before the field existed still belong
/// to that session, and dropping them would silently undercount old history.
///
/// A turn whose four counters sum to zero is dropped rather than kept at
/// zero — Orca's own rule. Those rows are stream scaffolding, and counting
/// them would inflate the turn count that the cache-reuse percentages divide
/// by, which is the sort of error that makes a healthy number look sick.
#[must_use]
pub fn parse_record(line: &str, fallback_session_id: Option<&str>) -> Option<SourceTurn> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str)? != "assistant" {
        return None;
    }
    let session_id = value
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .or(fallback_session_id)?
        .to_string();
    let timestamp = value
        .get("timestamp")
        .and_then(serde_json::Value::as_str)?
        .to_string();

    let message = value.get("message");
    let usage = message.and_then(|message| message.get("usage"));
    let count = |name: &str| {
        usage
            .and_then(|usage| usage.get(name))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    };
    let input_tokens = count("input_tokens");
    let output_tokens = count("output_tokens");
    let cache_read_tokens = count("cache_read_input_tokens");
    let cache_write_tokens = count("cache_creation_input_tokens");
    if input_tokens + output_tokens + cache_read_tokens + cache_write_tokens <= 0 {
        return None;
    }

    fn text<'a>(parent: Option<&'a serde_json::Value>, name: &str) -> Option<&'a str> {
        parent
            .and_then(|parent| parent.get(name))
            .and_then(serde_json::Value::as_str)
    }
    Some(SourceTurn {
        session_id,
        timestamp,
        model: text(message, "model").map(str::to_string),
        cwd: text(Some(&value), "cwd").map(str::to_string),
        git_branch: text(Some(&value), "gitBranch").map(str::to_string),
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        dedupe_key: dedupe_key(
            text(message, "id"),
            text(Some(&value), "requestId"),
            text(Some(&value), "uuid"),
        ),
    })
}

/// Collapses the repeats a stream writes, keeping the largest counter seen.
///
/// Claude Code emits an assistant row more than once for the same message as
/// the response streams, and the later copies carry the more complete usage.
/// Summing them would multiply a conversation's cost by however many times it
/// happened to flush; taking the maximum is Orca's rule and the only one that
/// survives a partial write.
#[must_use]
pub fn dedupe(turns: Vec<SourceTurn>) -> Vec<SourceTurn> {
    let mut at_key: HashMap<String, usize> = HashMap::new();
    let mut kept: Vec<SourceTurn> = Vec::with_capacity(turns.len());
    for turn in turns {
        if let Some(key) = turn.dedupe_key.clone()
            && let Some(&index) = at_key.get(&key)
        {
            let held = &mut kept[index];
            held.input_tokens = held.input_tokens.max(turn.input_tokens);
            held.output_tokens = held.output_tokens.max(turn.output_tokens);
            held.cache_read_tokens = held.cache_read_tokens.max(turn.cache_read_tokens);
            held.cache_write_tokens = held.cache_write_tokens.max(turn.cache_write_tokens);
            continue;
        }
        if let Some(key) = turn.dedupe_key.clone() {
            at_key.insert(key, kept.len());
        }
        kept.push(turn);
    }
    kept
}

/// A worktree this app manages, as the attribution needs to see it.
#[derive(Clone, Debug)]
pub struct WorktreeRef {
    pub repo_id: String,
    pub worktree_id: String,
    /// Already canonicalized by the caller — this module does no I/O.
    pub path: String,
    pub display_name: String,
}

/// A turn with its day and its project decided.
#[derive(Clone, Debug)]
pub struct AttributedTurn {
    pub turn: SourceTurn,
    pub day: String,
    pub project_key: String,
    pub project_label: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
}

/// The last two path segments — Orca's `getDefaultProjectLabel`.
#[must_use]
fn default_project_label(cwd: Option<&str>) -> String {
    let Some(cwd) = cwd else {
        return "Unknown location".to_string();
    };
    let normalized = cwd.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    match parts.len() {
        0 => cwd.to_string(),
        1 => parts[0].to_string(),
        _ => parts[parts.len() - 2..].join("/"),
    }
}

/// Whether `child` is `parent` or sits underneath it.
#[must_use]
fn contains_path(parent: &str, child: &str) -> bool {
    let parent = parent.trim_end_matches('/');
    let child = child.trim_end_matches('/');
    child == parent || child.starts_with(&format!("{parent}/"))
}

/// The worktree a path falls in, longest path first so a nested worktree wins
/// over the repository that contains it.
///
/// Public because the session panel groups by workspace with this same rule
/// ([`crate::vault::view`]): one conversation must not be filed under a
/// workspace in one pane and under a bare directory in the other, and the
/// nesting order is the part a second copy would get wrong.
#[must_use]
pub fn containing_worktree<'a>(cwd: &str, worktrees: &'a [WorktreeRef]) -> Option<&'a WorktreeRef> {
    let mut candidates: Vec<&WorktreeRef> = worktrees.iter().collect();
    candidates.sort_by_key(|worktree| std::cmp::Reverse(worktree.path.len()));
    candidates
        .into_iter()
        .find(|worktree| contains_path(&worktree.path, cwd))
}

/// Decides each turn's local day and project.
///
/// A turn whose timestamp cannot be read is dropped: it cannot be placed on
/// any day, and a row on no day would be counted by the totals while being
/// invisible in the chart they are supposed to explain.
#[must_use]
pub fn attribute(
    turns: Vec<SourceTurn>,
    worktrees: &[WorktreeRef],
    offset_minutes: i32,
) -> Vec<AttributedTurn> {
    turns
        .into_iter()
        .filter_map(|turn| {
            let placed = place(
                turn.cwd.as_deref(),
                &turn.timestamp,
                worktrees,
                offset_minutes,
            )?;
            Some(AttributedTurn {
                turn,
                day: placed.day,
                project_key: placed.project_key,
                project_label: placed.project_label,
                repo_id: placed.repo_id,
                worktree_id: placed.worktree_id,
            })
        })
        .collect()
}

/// Where and when one record happened, decided once for every provider.
#[derive(Clone, Debug)]
pub struct Placement {
    pub day: String,
    pub project_key: String,
    pub project_label: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
}

/// The local day a stamp falls on and the project a directory belongs to.
///
/// Shared rather than written once per vendor: the two ledgers read different
/// file formats, but "which day is this" and "whose project is this" are the
/// same two questions, and two copies of the answer is how a Codex turn and a
/// Claude turn on the same afternoon end up filed under different days.
///
/// `None` when the stamp cannot be read — a record on no day would be counted
/// by totals it can never appear in.
#[must_use]
pub fn place(
    cwd: Option<&str>,
    timestamp: &str,
    worktrees: &[WorktreeRef],
    offset_minutes: i32,
) -> Option<Placement> {
    let day = iso_date_of(epoch_ms_of_iso(timestamp)?, offset_minutes);
    let mut placed = Placement {
        day,
        project_key: "unscoped".to_string(),
        project_label: default_project_label(cwd),
        repo_id: None,
        worktree_id: None,
    };
    if let Some(cwd) = cwd {
        let cwd = cwd.replace('\\', "/");
        if let Some(worktree) = containing_worktree(&cwd, worktrees) {
            placed.repo_id = Some(worktree.repo_id.clone());
            placed.worktree_id = Some(worktree.worktree_id.clone());
            placed.project_key = format!("worktree:{}", worktree.worktree_id);
            placed.project_label = worktree.display_name.clone();
        } else {
            placed.project_key = format!("cwd:{cwd}");
        }
    }
    Some(placed)
}

/// Dollars per million tokens for one model, with the long-context tier some
/// of them bill above a threshold.
#[derive(Clone, Copy)]
struct Pricing {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    /// `None` for a model that bills one flat rate at every context length.
    long_context: Option<LongContext>,
}

#[derive(Clone, Copy)]
struct LongContext {
    threshold_tokens: f64,
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

/// The tier Sonnet 4-series models bill above 200k tokens of context.
const SONNET_LONG_CONTEXT: LongContext = LongContext {
    threshold_tokens: 200_000.0,
    input: 6.0,
    output: 22.5,
    cache_read: 0.6,
    cache_write: 7.5,
};

/// Orca's `MODEL_PRICING`, in dollars per million tokens.
///
/// Sonnet 5 bills its full window at flat rates, so it carries no long-context
/// tier; the figures are the standard rates rather than the introductory ones,
/// because this table has no date dimension to expire them with.
fn pricing_of(model: &str) -> Option<Pricing> {
    let flat = |input, output, cache_read, cache_write| {
        Some(Pricing {
            input,
            output,
            cache_read,
            cache_write,
            long_context: None,
        })
    };
    let sonnet_4 = || {
        Some(Pricing {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
            long_context: Some(SONNET_LONG_CONTEXT),
        })
    };
    match model {
        "claude-fable-5-1" => flat(10.0, 50.0, 0.25, 12.5),
        "claude-fable-5" => flat(10.0, 50.0, 1.0, 12.5),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6"
        | "claude-opus-4-5" => flat(5.0, 25.0, 0.5, 6.25),
        "claude-opus-4-1" | "claude-opus-4" => flat(15.0, 75.0, 1.5, 18.75),
        "claude-sonnet-5" => flat(3.0, 15.0, 0.3, 3.75),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" | "claude-sonnet-4" => sonnet_4(),
        "claude-sonnet-3-7" | "claude-sonnet-3-5" => flat(3.0, 15.0, 0.3, 3.75),
        "claude-haiku-4-5" => flat(1.0, 5.0, 0.1, 1.25),
        "claude-haiku-3-5" => flat(0.8, 4.0, 0.08, 1.0),
        "claude-haiku-3" => flat(0.25, 1.25, 0.03, 0.3),
        _ => None,
    }
}

/// Whether a model id names this family at this version.
///
/// The version has to end at a non-digit or the string's end, so `opus-4-1`
/// does not answer for a query about `opus-4`.
fn has_version(model: &str, family: &str, version: &str) -> bool {
    let normalized = model.replace('.', "-");
    let needle = format!("{family}-{version}");
    let mut from = 0;
    while let Some(at) = normalized[from..].find(&needle) {
        let at = from + at;
        let after = at + needle.len();
        match normalized.as_bytes().get(after) {
            None => return true,
            Some(byte) if !byte.is_ascii_digit() => return true,
            _ => from = at + 1,
        }
    }
    false
}

/// The pre-4.5 Opus 4 releases, which bill at the old high rate.
fn is_legacy_opus_4(model: &str) -> bool {
    let normalized = model.replace('.', "-");
    let Some(at) = normalized.find("opus-4") else {
        return false;
    };
    let rest = &normalized[at + "opus-4".len()..];
    rest.is_empty()
        || rest == "-thinking"
        || (rest.starts_with("-20")
            && rest.len() >= 9
            && rest[1..9].bytes().all(|b| b.is_ascii_digit()))
        || (rest.starts_with("@20")
            && rest.len() >= 9
            && rest[1..9].bytes().all(|b| b.is_ascii_digit()))
        || (rest.starts_with("-20") && rest.ends_with("-thinking"))
}

/// The price-table key a transcript's model id maps to, or `None` when this
/// table has never heard of it.
///
/// `None` matters: an unknown model contributes no cost at all rather than a
/// guessed one, and the pane says `n/a` instead of a number nobody can check.
#[must_use]
pub fn pricing_key(model: Option<&str>) -> Option<String> {
    let model = model?;
    let lower = model
        .to_lowercase()
        .trim()
        .trim_start_matches("anthropic/")
        .trim_start_matches("anthropic:")
        .to_string();
    let alias = match lower.as_str() {
        "model_placeholder_m26" => Some("claude-opus-4-6"),
        "model_placeholder_m35" => Some("claude-sonnet-4-6"),
        "claude-opus-4.8" | "claude-opus-4.8-thinking" | "claude-opus-4-8-thinking" => {
            Some("claude-opus-4-8")
        }
        "claude-opus-4.6" | "claude-opus-4.6-thinking" | "claude-opus-4-6-thinking" => {
            Some("claude-opus-4-6")
        }
        "claude-sonnet-4.6" | "claude-sonnet-4.6-thinking" | "claude-sonnet-4-6-thinking" => {
            Some("claude-sonnet-4-6")
        }
        _ => None,
    };
    if let Some(alias) = alias {
        return Some(alias.to_string());
    }
    for (family, version, key) in [
        ("fable", "5-1", "claude-fable-5-1"),
        ("fable", "5", "claude-fable-5"),
        ("opus", "5", "claude-opus-5"),
        ("opus", "4-8", "claude-opus-4-8"),
        ("opus", "4-7", "claude-opus-4-7"),
        ("opus", "4-6", "claude-opus-4-6"),
        ("opus", "4-5", "claude-opus-4-5"),
        ("opus", "4-1", "claude-opus-4-1"),
    ] {
        if has_version(&lower, family, version) {
            return Some(key.to_string());
        }
    }
    if is_legacy_opus_4(&lower) {
        return Some("claude-opus-4".to_string());
    }
    if lower.contains("opus-4") {
        // A point release this table has not met yet bills at the current low
        // Opus rate, not the legacy one — overbilling an unknown id is the
        // worse of the two errors.
        return Some("claude-opus-4-8".to_string());
    }
    for (family, version, key) in [
        ("sonnet", "5", "claude-sonnet-5"),
        ("sonnet", "4-6", "claude-sonnet-4-6"),
        ("sonnet", "4-5", "claude-sonnet-4-5"),
    ] {
        if has_version(&lower, family, version) {
            return Some(key.to_string());
        }
    }
    if lower.contains("sonnet-4") {
        return Some("claude-sonnet-4-6".to_string());
    }
    if lower.contains("sonnet-3-7") || lower.contains("sonnet-3.7") {
        return Some("claude-sonnet-3-7".to_string());
    }
    // Version-first ids like `claude-3-5-sonnet-20241022` are still in old
    // logs on disk; matching them keeps their cost out of the silently-dropped
    // pile.
    for needle in ["sonnet-3-5", "sonnet-3.5", "3-5-sonnet", "3.5-sonnet"] {
        if lower.contains(needle) {
            return Some("claude-sonnet-3-5".to_string());
        }
    }
    if lower.contains("haiku-4-5") {
        return Some("claude-haiku-4-5".to_string());
    }
    for needle in ["haiku-3-5", "haiku-3.5", "3-5-haiku", "3.5-haiku"] {
        if lower.contains(needle) {
            return Some("claude-haiku-3-5".to_string());
        }
    }
    if lower.contains("haiku-3") {
        return Some("claude-haiku-3".to_string());
    }
    None
}

/// Splits a token count across a threshold and prices each half.
fn tiered(tokens: f64, base: f64, above: Option<(f64, f64)>) -> f64 {
    let Some((threshold, above_price)) = above else {
        return tokens * base;
    };
    let below = tokens.min(threshold);
    let over = (tokens - threshold).max(0.0);
    below * base + over * above_price
}

/// What these tokens would have cost on the API, or `None` for a model with
/// no price on file.
#[must_use]
pub fn estimate_cost_usd(
    model: Option<&str>,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
) -> Option<f64> {
    let pricing = pricing_of(&pricing_key(model)?)?;
    let tier = |pick: fn(&LongContext) -> f64| {
        pricing
            .long_context
            .map(|long| (long.threshold_tokens, pick(&long)))
    };
    let total = tiered(input_tokens as f64, pricing.input, tier(|long| long.input))
        + tiered(
            output_tokens as f64,
            pricing.output,
            tier(|long| long.output),
        )
        + tiered(
            cache_read_tokens as f64,
            pricing.cache_read,
            tier(|long| long.cache_read),
        )
        + tiered(
            cache_write_tokens as f64,
            pricing.cache_write,
            tier(|long| long.cache_write),
        );
    Some(total / 1_000_000.0)
}

/// Where one session spent its turns. A session that moved between checkouts
/// has one of these per place.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocationBreakdown {
    pub location_key: String,
    pub project_label: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub turn_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

/// One conversation, totalled.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub first_timestamp: String,
    pub last_timestamp: String,
    pub model: Option<String>,
    pub last_cwd: Option<String>,
    pub last_git_branch: Option<String>,
    pub turn_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_cache_write_tokens: i64,
    pub location_breakdown: Vec<LocationBreakdown>,
}

/// One day of one model in one project.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyAggregate {
    pub day: String,
    pub model: Option<String>,
    pub project_key: String,
    pub project_label: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub turn_count: i64,
    pub zero_cache_read_turn_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

/// Everything one scan learned, before any scope or range is applied.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    pub sessions: Vec<Session>,
    pub daily_aggregates: Vec<DailyAggregate>,
}

/// Rolls attributed turns into sessions and per-day rows.
#[must_use]
pub fn aggregate(turns: Vec<AttributedTurn>) -> Ledger {
    let mut sessions: BTreeMap<String, Session> = BTreeMap::new();
    let mut daily: BTreeMap<String, DailyAggregate> = BTreeMap::new();

    for attributed in turns {
        let turn = &attributed.turn;
        let session = sessions
            .entry(turn.session_id.clone())
            .or_insert_with(|| Session {
                session_id: turn.session_id.clone(),
                first_timestamp: turn.timestamp.clone(),
                last_timestamp: turn.timestamp.clone(),
                model: turn.model.clone(),
                last_cwd: turn.cwd.clone(),
                last_git_branch: turn.git_branch.clone(),
                turn_count: 0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cache_read_tokens: 0,
                total_cache_write_tokens: 0,
                location_breakdown: Vec::new(),
            });
        if turn.timestamp < session.first_timestamp {
            session.first_timestamp = turn.timestamp.clone();
        }
        if turn.timestamp > session.last_timestamp {
            session.last_timestamp = turn.timestamp.clone();
            session.last_cwd = turn.cwd.clone();
            session.last_git_branch = turn.git_branch.clone();
        }
        if turn.model.is_some() {
            session.model = turn.model.clone();
        }
        session.turn_count += 1;
        session.total_input_tokens += turn.input_tokens;
        session.total_output_tokens += turn.output_tokens;
        session.total_cache_read_tokens += turn.cache_read_tokens;
        session.total_cache_write_tokens += turn.cache_write_tokens;

        match session
            .location_breakdown
            .iter_mut()
            .find(|entry| entry.location_key == attributed.project_key)
        {
            Some(location) => {
                location.turn_count += 1;
                location.input_tokens += turn.input_tokens;
                location.output_tokens += turn.output_tokens;
                location.cache_read_tokens += turn.cache_read_tokens;
                location.cache_write_tokens += turn.cache_write_tokens;
            }
            None => session.location_breakdown.push(LocationBreakdown {
                location_key: attributed.project_key.clone(),
                project_label: attributed.project_label.clone(),
                repo_id: attributed.repo_id.clone(),
                worktree_id: attributed.worktree_id.clone(),
                turn_count: 1,
                input_tokens: turn.input_tokens,
                output_tokens: turn.output_tokens,
                cache_read_tokens: turn.cache_read_tokens,
                cache_write_tokens: turn.cache_write_tokens,
            }),
        }

        let model_key = turn.model.clone().unwrap_or_else(|| "unknown".to_string());
        let daily_key = format!(
            "{}::{model_key}::{}",
            attributed.day, attributed.project_key
        );
        let row = daily.entry(daily_key).or_insert_with(|| DailyAggregate {
            day: attributed.day.clone(),
            model: turn.model.clone(),
            project_key: attributed.project_key.clone(),
            project_label: attributed.project_label.clone(),
            repo_id: attributed.repo_id.clone(),
            worktree_id: attributed.worktree_id.clone(),
            turn_count: 0,
            zero_cache_read_turn_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        });
        row.turn_count += 1;
        if turn.cache_read_tokens == 0 {
            row.zero_cache_read_turn_count += 1;
        }
        row.input_tokens += turn.input_tokens;
        row.output_tokens += turn.output_tokens;
        row.cache_read_tokens += turn.cache_read_tokens;
        row.cache_write_tokens += turn.cache_write_tokens;
    }

    let mut sessions: Vec<Session> = sessions.into_values().collect();
    for session in &mut sessions {
        session.location_breakdown.sort_by(|left, right| {
            (right.input_tokens + right.output_tokens)
                .cmp(&(left.input_tokens + left.output_tokens))
        });
    }
    // Newest first: the recent-sessions table takes the head of this list.
    sessions.sort_by(|left, right| right.last_timestamp.cmp(&left.last_timestamp));

    let mut daily_aggregates: Vec<DailyAggregate> = daily.into_values().collect();
    daily_aggregates.sort_by(|left, right| {
        left.day
            .cmp(&right.day)
            .then_with(|| left.project_label.cmp(&right.project_label))
    });

    Ledger {
        sessions,
        daily_aggregates,
    }
}

/// Merges one file's ledger into a running one.
///
/// A session's turns can be split across files — a resume writes a new
/// transcript that continues the same conversation — so the totals add and
/// the timestamps take the outer bounds.
pub fn merge(into: &mut Ledger, from: Ledger) {
    let mut at_session: HashMap<String, usize> = into
        .sessions
        .iter()
        .enumerate()
        .map(|(index, session)| (session.session_id.clone(), index))
        .collect();
    for session in from.sessions {
        let Some(&index) = at_session.get(&session.session_id) else {
            at_session.insert(session.session_id.clone(), into.sessions.len());
            into.sessions.push(session);
            continue;
        };
        let held = &mut into.sessions[index];
        if session.first_timestamp < held.first_timestamp {
            held.first_timestamp = session.first_timestamp.clone();
        }
        if session.last_timestamp > held.last_timestamp {
            held.last_timestamp = session.last_timestamp.clone();
            held.last_cwd = session.last_cwd.clone();
            held.last_git_branch = session.last_git_branch.clone();
        }
        if session.model.is_some() {
            held.model = session.model.clone();
        }
        held.turn_count += session.turn_count;
        held.total_input_tokens += session.total_input_tokens;
        held.total_output_tokens += session.total_output_tokens;
        held.total_cache_read_tokens += session.total_cache_read_tokens;
        held.total_cache_write_tokens += session.total_cache_write_tokens;
        for location in session.location_breakdown {
            match held
                .location_breakdown
                .iter_mut()
                .find(|entry| entry.location_key == location.location_key)
            {
                Some(existing) => {
                    existing.turn_count += location.turn_count;
                    existing.input_tokens += location.input_tokens;
                    existing.output_tokens += location.output_tokens;
                    existing.cache_read_tokens += location.cache_read_tokens;
                    existing.cache_write_tokens += location.cache_write_tokens;
                }
                None => held.location_breakdown.push(location),
            }
        }
    }

    let mut at_daily: HashMap<String, usize> = into
        .daily_aggregates
        .iter()
        .enumerate()
        .map(|(index, row)| {
            (
                format!(
                    "{}::{}::{}",
                    row.day,
                    row.model.clone().unwrap_or_else(|| "unknown".to_string()),
                    row.project_key
                ),
                index,
            )
        })
        .collect();
    for row in from.daily_aggregates {
        let key = format!(
            "{}::{}::{}",
            row.day,
            row.model.clone().unwrap_or_else(|| "unknown".to_string()),
            row.project_key
        );
        let Some(&index) = at_daily.get(&key) else {
            at_daily.insert(key, into.daily_aggregates.len());
            into.daily_aggregates.push(row);
            continue;
        };
        let held = &mut into.daily_aggregates[index];
        held.turn_count += row.turn_count;
        held.zero_cache_read_turn_count += row.zero_cache_read_turn_count;
        held.input_tokens += row.input_tokens;
        held.output_tokens += row.output_tokens;
        held.cache_read_tokens += row.cache_read_tokens;
        held.cache_write_tokens += row.cache_write_tokens;
    }
}

/// Puts a merged ledger back in the order the panes read it in.
pub fn finalize(ledger: &mut Ledger) {
    for session in &mut ledger.sessions {
        session.location_breakdown.sort_by(|left, right| {
            (right.input_tokens + right.output_tokens)
                .cmp(&(left.input_tokens + left.output_tokens))
        });
    }
    ledger
        .sessions
        .sort_by(|left, right| right.last_timestamp.cmp(&left.last_timestamp));
    ledger.daily_aggregates.sort_by(|left, right| {
        left.day
            .cmp(&right.day)
            .then_with(|| left.project_label.cmp(&right.project_label))
    });
}

/// The headline figures, for one scope and range.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub scope: Scope,
    pub range: Range,
    pub sessions: i64,
    pub turns: i64,
    pub zero_cache_read_turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_reuse_rate: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    pub top_model: Option<String>,
    pub top_project: Option<String>,
    pub has_any_data: bool,
}

/// One bar of the daily chart.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyPoint {
    pub day: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

/// One row of a by-model or by-project list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BreakdownRow {
    pub key: String,
    pub label: String,
    pub sessions: i64,
    pub turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub estimated_cost_usd: Option<f64>,
}

/// One row of the recent-sessions table.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRow {
    pub session_id: String,
    pub last_active_at: String,
    pub duration_minutes: i64,
    pub project_label: String,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

/// Which of the two lists a breakdown is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakdownKind {
    Model,
    Project,
}

/// Everything the pane draws for one scope and range.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub summary: Summary,
    pub daily: Vec<DailyPoint>,
    pub model_breakdown: Vec<BreakdownRow>,
    pub project_breakdown: Vec<BreakdownRow>,
    pub recent_sessions: Vec<SessionRow>,
}

/// The day rows this scope and range keep.
fn filtered_daily<'a>(
    ledger: &'a Ledger,
    scope: Scope,
    cutoff: Option<&str>,
) -> Vec<&'a DailyAggregate> {
    ledger
        .daily_aggregates
        .iter()
        .filter(|row| {
            if let Some(cutoff) = cutoff
                && row.day.as_str() < cutoff
            {
                return false;
            }
            !(scope == Scope::Zerocode && row.worktree_id.is_none())
        })
        .collect()
}

/// The sessions this scope and range keep.
///
/// The session's day is derived the same way the day rows were, because a
/// session filtered by UTC while the chart is filtered by local time is a
/// table and a chart that disagree at every midnight.
fn filtered_sessions<'a>(
    ledger: &'a Ledger,
    scope: Scope,
    cutoff: Option<&str>,
    offset_minutes: i32,
) -> Vec<&'a Session> {
    ledger
        .sessions
        .iter()
        .filter(|session| {
            let Some(epoch_ms) = epoch_ms_of_iso(&session.last_timestamp) else {
                return false;
            };
            if let Some(cutoff) = cutoff
                && iso_date_of(epoch_ms, offset_minutes).as_str() < cutoff
            {
                return false;
            }
            if scope == Scope::Zerocode {
                return session
                    .location_breakdown
                    .iter()
                    .any(|entry| entry.worktree_id.is_some());
            }
            true
        })
        .collect()
}

/// The label a session's row shows for where it ran.
#[must_use]
fn session_project_label(locations: &[LocationBreakdown]) -> String {
    match locations.len() {
        0 => "Unknown location".to_string(),
        1 => locations[0].project_label.clone(),
        _ => "Multiple locations".to_string(),
    }
}

/// Builds every figure the pane draws.
///
/// One entry point rather than four, because the summary, the chart, the two
/// breakdowns and the table must all be computed from the SAME filtered set —
/// four callers each re-deriving the cutoff is four chances to disagree about
/// which day the week starts.
#[must_use]
pub fn report(
    ledger: &Ledger,
    scope: Scope,
    range: Range,
    now_ms: i64,
    offset_minutes: i32,
) -> Report {
    let cutoff = range_cutoff(range, now_ms, offset_minutes);
    let cutoff = cutoff.as_deref();
    let daily_rows = filtered_daily(ledger, scope, cutoff);
    let sessions = filtered_sessions(ledger, scope, cutoff, offset_minutes);

    let mut summary = Summary {
        scope,
        range,
        sessions: sessions.len() as i64,
        turns: 0,
        zero_cache_read_turns: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cache_reuse_rate: None,
        estimated_cost_usd: None,
        top_model: None,
        top_project: None,
        has_any_data: !sessions.is_empty() || !daily_rows.is_empty(),
    };
    let mut by_model: BTreeMap<String, i64> = BTreeMap::new();
    let mut by_project: BTreeMap<String, i64> = BTreeMap::new();
    let mut cost = 0.0;
    let mut any_billable = false;
    for row in &daily_rows {
        summary.input_tokens += row.input_tokens;
        summary.output_tokens += row.output_tokens;
        summary.cache_read_tokens += row.cache_read_tokens;
        summary.cache_write_tokens += row.cache_write_tokens;
        summary.turns += row.turn_count;
        summary.zero_cache_read_turns += row.zero_cache_read_turn_count;
        let tokens = row.input_tokens + row.output_tokens;
        *by_model
            .entry(
                row.model
                    .clone()
                    .unwrap_or_else(|| "Unknown model".to_string()),
            )
            .or_insert(0) += tokens;
        *by_project.entry(row.project_label.clone()).or_insert(0) += tokens;
        if let Some(row_cost) = estimate_cost_usd(
            row.model.as_deref(),
            row.input_tokens,
            row.output_tokens,
            row.cache_read_tokens,
            row.cache_write_tokens,
        ) {
            any_billable = true;
            cost += row_cost;
        }
    }
    let heaviest = |counts: BTreeMap<String, i64>| {
        counts
            .into_iter()
            .max_by(|left, right| left.1.cmp(&right.1))
            .map(|(label, _)| label)
    };
    summary.top_model = heaviest(by_model);
    summary.top_project = heaviest(by_project);
    if summary.input_tokens + summary.cache_read_tokens > 0 {
        summary.cache_reuse_rate = Some(
            summary.cache_read_tokens as f64
                / (summary.input_tokens + summary.cache_read_tokens) as f64,
        );
    }
    if any_billable {
        summary.estimated_cost_usd = Some(cost);
    }

    let mut by_day: BTreeMap<String, DailyPoint> = BTreeMap::new();
    for row in &daily_rows {
        let point = by_day.entry(row.day.clone()).or_insert_with(|| DailyPoint {
            day: row.day.clone(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        });
        point.input_tokens += row.input_tokens;
        point.output_tokens += row.output_tokens;
        point.cache_read_tokens += row.cache_read_tokens;
        point.cache_write_tokens += row.cache_write_tokens;
    }

    Report {
        summary,
        daily: by_day.into_values().collect(),
        model_breakdown: breakdown(&daily_rows, &sessions, scope, BreakdownKind::Model),
        project_breakdown: breakdown(&daily_rows, &sessions, scope, BreakdownKind::Project),
        recent_sessions: recent_sessions(&sessions, scope, 12),
    }
}

/// One of the two lists under the chart.
#[must_use]
fn breakdown(
    daily_rows: &[&DailyAggregate],
    sessions: &[&Session],
    scope: Scope,
    kind: BreakdownKind,
) -> Vec<BreakdownRow> {
    let mut rows: BTreeMap<String, BreakdownRow> = BTreeMap::new();
    for row in daily_rows {
        let (key, label) = match kind {
            BreakdownKind::Model => (
                row.model.clone().unwrap_or_else(|| "unknown".to_string()),
                row.model
                    .clone()
                    .unwrap_or_else(|| "Unknown model".to_string()),
            ),
            BreakdownKind::Project => (row.project_key.clone(), row.project_label.clone()),
        };
        let held = rows.entry(key.clone()).or_insert_with(|| BreakdownRow {
            key,
            label,
            sessions: 0,
            turns: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            estimated_cost_usd: None,
        });
        held.turns += row.turn_count;
        held.input_tokens += row.input_tokens;
        held.output_tokens += row.output_tokens;
        held.cache_read_tokens += row.cache_read_tokens;
        held.cache_write_tokens += row.cache_write_tokens;
    }

    for session in sessions {
        match kind {
            BreakdownKind::Model => {
                let key = session
                    .model
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                if let Some(row) = rows.get_mut(&key) {
                    row.sessions += 1;
                }
            }
            BreakdownKind::Project => {
                let mut seen: HashSet<&str> = HashSet::new();
                for location in &session.location_breakdown {
                    if scope == Scope::Zerocode && location.worktree_id.is_none() {
                        continue;
                    }
                    if !seen.insert(location.location_key.as_str()) {
                        continue;
                    }
                    if let Some(row) = rows.get_mut(&location.location_key) {
                        row.sessions += 1;
                    }
                }
            }
        }
    }

    let mut rows: Vec<BreakdownRow> = rows.into_values().collect();
    if kind == BreakdownKind::Model {
        for row in &mut rows {
            row.estimated_cost_usd = estimate_cost_usd(
                Some(&row.key),
                row.input_tokens,
                row.output_tokens,
                row.cache_read_tokens,
                row.cache_write_tokens,
            );
        }
    }
    rows.sort_by(|left, right| {
        (right.input_tokens + right.output_tokens).cmp(&(left.input_tokens + left.output_tokens))
    });
    rows
}

/// The head of the session list, totalled over the locations this scope keeps.
#[must_use]
fn recent_sessions(sessions: &[&Session], scope: Scope, limit: usize) -> Vec<SessionRow> {
    sessions
        .iter()
        .take(limit)
        .map(|session| {
            let scoped: Vec<&LocationBreakdown> = {
                let matching: Vec<&LocationBreakdown> = session
                    .location_breakdown
                    .iter()
                    .filter(|entry| scope == Scope::All || entry.worktree_id.is_some())
                    .collect();
                if matching.is_empty() {
                    session.location_breakdown.iter().collect()
                } else {
                    matching
                }
            };
            let mut row = SessionRow {
                session_id: session.session_id.clone(),
                last_active_at: session.last_timestamp.clone(),
                duration_minutes: 0,
                project_label: session_project_label(
                    &scoped
                        .iter()
                        .map(|entry| (*entry).clone())
                        .collect::<Vec<_>>(),
                ),
                branch: session.last_git_branch.clone(),
                model: session.model.clone(),
                turns: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            };
            for entry in scoped {
                row.turns += entry.turn_count;
                row.input_tokens += entry.input_tokens;
                row.output_tokens += entry.output_tokens;
                row.cache_read_tokens += entry.cache_read_tokens;
                row.cache_write_tokens += entry.cache_write_tokens;
            }
            if let (Some(first), Some(last)) = (
                epoch_ms_of_iso(&session.first_timestamp),
                epoch_ms_of_iso(&session.last_timestamp),
            ) {
                row.duration_minutes = (((last - first) as f64) / 60_000.0).round().max(0.0) as i64;
            }
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seoul, which is where these figures are read — a positive offset large
    /// enough that a UTC evening is already the next local day.
    const KST: i32 = 9 * 60;

    fn line(fields: &str) -> String {
        format!("{{\"type\":\"assistant\",{fields}}}")
    }

    fn turn(session: &str, timestamp: &str, cwd: Option<&str>, input: i64) -> SourceTurn {
        SourceTurn {
            session_id: session.to_string(),
            timestamp: timestamp.to_string(),
            model: Some("claude-opus-5".to_string()),
            cwd: cwd.map(str::to_string),
            git_branch: Some("main".to_string()),
            input_tokens: input,
            output_tokens: 10,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            dedupe_key: None,
        }
    }

    /// Only billable assistant rows become turns.
    ///
    /// The three rejections are each a real line in a transcript: a user row,
    /// an assistant row whose counters are all zero (stream scaffolding), and
    /// a row with no timestamp to place on a day.
    #[test]
    fn a_turn_is_an_assistant_row_that_spent_something() {
        let good = line(
            r#""sessionId":"s1","timestamp":"2026-08-19T01:00:00Z","cwd":"/w/p","gitBranch":"main","message":{"model":"claude-opus-5","usage":{"input_tokens":5,"output_tokens":7}}"#,
        );
        let parsed = parse_record(&good, None).expect("a billable assistant row is a turn");
        assert_eq!(parsed.session_id, "s1");
        assert_eq!(parsed.input_tokens, 5);
        assert_eq!(parsed.output_tokens, 7);
        assert_eq!(parsed.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(parsed.cwd.as_deref(), Some("/w/p"));

        assert!(
            parse_record(
                r#"{"type":"user","sessionId":"s1","timestamp":"2026-08-19T01:00:00Z"}"#,
                None
            )
            .is_none(),
            "a user row was counted as usage"
        );
        let empty = line(
            r#""sessionId":"s1","timestamp":"2026-08-19T01:00:00Z","message":{"usage":{"input_tokens":0,"output_tokens":0}}"#,
        );
        assert!(
            parse_record(&empty, None).is_none(),
            "a zero-token row was counted, which inflates the turn count the \
             reuse percentages divide by"
        );
        let undated = line(r#""sessionId":"s1","message":{"usage":{"output_tokens":3}}"#);
        assert!(
            parse_record(&undated, None).is_none(),
            "a row with no timestamp was kept"
        );
        assert!(
            parse_record("not json", None).is_none(),
            "a broken line was not skipped"
        );
    }

    /// A row with no `sessionId` still belongs to the file it was found in.
    #[test]
    fn a_session_id_falls_back_to_the_file_it_was_found_in() {
        let text =
            line(r#""timestamp":"2026-08-19T01:00:00Z","message":{"usage":{"output_tokens":3}}"#);
        assert!(
            parse_record(&text, None).is_none(),
            "a row with no session and no file name was kept anyway"
        );
        let parsed = parse_record(&text, Some("file-stem")).expect("the file names the session");
        assert_eq!(parsed.session_id, "file-stem");
    }

    /// The dedupe identity, in the original's own order of preference.
    ///
    /// A fork rewrites `sessionId` but keeps the message and request ids, so
    /// that pair is the strongest claim; `uuid` carries rows old enough to
    /// predate `requestId`.
    #[test]
    fn a_turn_is_identified_by_the_strongest_id_it_carries() {
        let key_of = |fields: &str| {
            let text = line(&format!(
                r#""sessionId":"s","timestamp":"2026-08-19T01:00:00Z",{fields}"#
            ));
            parse_record(&text, None)
                .expect("a billable row")
                .dedupe_key
        };
        assert_eq!(
            key_of(
                r#""requestId":"req","uuid":"u","message":{"id":"msg","usage":{"output_tokens":1}}"#
            )
            .as_deref(),
            Some("msg:req"),
            "the message/request pair lost to a weaker id"
        );
        assert_eq!(
            key_of(r#""uuid":"u","message":{"id":"msg","usage":{"output_tokens":1}}"#).as_deref(),
            Some("msg:msg"),
            "a row with a message id but no request id fell through to its uuid"
        );
        assert_eq!(
            key_of(r#""uuid":"u","message":{"usage":{"output_tokens":1}}"#).as_deref(),
            Some("uuid:u")
        );
        assert_eq!(
            key_of(r#""message":{"usage":{"output_tokens":1}}"#),
            None,
            "a row with no id at all claimed one, so unrelated turns collapse"
        );
    }

    /// Repeats of one message keep the largest counters, never their sum.
    ///
    /// Claude Code flushes an assistant row more than once as the answer
    /// streams. Adding those copies would multiply a conversation's cost by
    /// however many times it happened to flush.
    #[test]
    fn a_repeated_message_is_counted_once_at_its_fullest() {
        let repeat = |input: i64, output: i64| SourceTurn {
            dedupe_key: Some("msg:req".to_string()),
            input_tokens: input,
            output_tokens: output,
            ..turn("s", "2026-08-19T01:00:00Z", Some("/w/p"), 0)
        };
        let kept = dedupe(vec![repeat(5, 10), repeat(5, 40), repeat(5, 25)]);
        assert_eq!(
            kept.len(),
            1,
            "the stream's repeats were counted as separate turns"
        );
        assert_eq!(kept[0].output_tokens, 40, "the fullest copy did not win");
        assert_eq!(kept[0].input_tokens, 5);

        // A turn with no key cannot be matched to anything, so it always stands.
        let unkeyed = dedupe(vec![
            turn("s", "2026-08-19T01:00:00Z", None, 1),
            turn("s", "2026-08-19T01:00:00Z", None, 1),
        ]);
        assert_eq!(
            unkeyed.len(),
            2,
            "unidentifiable turns were collapsed into one"
        );
    }

    /// The day is the person's, not UTC's.
    #[test]
    fn a_turn_lands_on_the_local_day() {
        let evening = vec![turn("s", "2026-08-18T20:00:00Z", None, 1)];
        let attributed = attribute(evening, &[], KST);
        assert_eq!(
            attributed[0].day, "2026-08-19",
            "a UTC evening was filed on the UTC day, so the chart disagrees \
             with the person's calendar every night"
        );
        assert_eq!(
            attribute(vec![turn("s", "2026-08-18T20:00:00Z", None, 1)], &[], 0)[0].day,
            "2026-08-18"
        );
        assert!(
            attribute(vec![turn("s", "nonsense", None, 1)], &[], KST).is_empty(),
            "a turn whose timestamp cannot be read was kept and would be \
             counted by totals it can never appear in"
        );
    }

    /// A turn is attributed to the deepest worktree that contains it.
    #[test]
    fn the_nearest_worktree_claims_the_turn() {
        let worktrees = vec![
            WorktreeRef {
                repo_id: "repo".to_string(),
                worktree_id: "root".to_string(),
                path: "/w/repo".to_string(),
                display_name: "repo".to_string(),
            },
            WorktreeRef {
                repo_id: "repo".to_string(),
                worktree_id: "feature".to_string(),
                path: "/w/repo/trees/feature".to_string(),
                display_name: "repo · feature".to_string(),
            },
        ];
        let inside = attribute(
            vec![turn(
                "s",
                "2026-08-19T01:00:00Z",
                Some("/w/repo/trees/feature/src"),
                1,
            )],
            &worktrees,
            KST,
        );
        assert_eq!(
            inside[0].worktree_id.as_deref(),
            Some("feature"),
            "the containing repository claimed a turn that belongs to the \
             worktree nested inside it"
        );
        assert_eq!(inside[0].project_key, "worktree:feature");
        assert_eq!(inside[0].project_label, "repo · feature");

        let outside = attribute(
            vec![turn(
                "s",
                "2026-08-19T01:00:00Z",
                Some("/elsewhere/other/deep"),
                1,
            )],
            &worktrees,
            KST,
        );
        assert_eq!(
            outside[0].worktree_id, None,
            "an unmanaged path claimed a worktree"
        );
        assert_eq!(outside[0].project_key, "cwd:/elsewhere/other/deep");
        assert_eq!(
            outside[0].project_label, "other/deep",
            "an unmanaged path lost its last-two-segments label"
        );

        // A path that merely shares a prefix is not inside.
        let sibling = attribute(
            vec![turn("s", "2026-08-19T01:00:00Z", Some("/w/repository"), 1)],
            &worktrees,
            KST,
        );
        assert_eq!(
            sibling[0].worktree_id, None,
            "`/w/repository` was read as being inside `/w/repo`"
        );
    }
    /// A session's totals, its places, and the day rows it wrote.
    #[test]
    fn a_session_totals_its_turns_and_names_where_they_ran() {
        let turns = attribute(
            vec![
                turn("s1", "2026-08-19T01:00:00Z", Some("/w/a"), 10),
                SourceTurn {
                    cache_read_tokens: 400,
                    ..turn("s1", "2026-08-19T02:00:00Z", Some("/w/a"), 20)
                },
                turn("s1", "2026-08-19T03:00:00Z", Some("/w/b"), 30),
            ],
            &[],
            KST,
        );
        let ledger = aggregate(turns);

        assert_eq!(ledger.sessions.len(), 1);
        let session = &ledger.sessions[0];
        assert_eq!(session.turn_count, 3);
        assert_eq!(session.total_input_tokens, 60);
        assert_eq!(session.total_output_tokens, 30);
        assert_eq!(session.first_timestamp, "2026-08-19T01:00:00Z");
        assert_eq!(
            session.last_timestamp, "2026-08-19T03:00:00Z",
            "the session's last activity is not its latest turn"
        );
        assert_eq!(
            session.last_cwd.as_deref(),
            Some("/w/b"),
            "the session reports a directory it had already left"
        );
        assert_eq!(
            session.location_breakdown.len(),
            2,
            "the two places were merged into one"
        );

        // Two projects on one day, so two day rows — and the zero-cache-read
        // count is per turn, not per row.
        assert_eq!(ledger.daily_aggregates.len(), 2);
        let total_zero: i64 = ledger
            .daily_aggregates
            .iter()
            .map(|row| row.zero_cache_read_turn_count)
            .sum();
        assert_eq!(
            total_zero, 2,
            "the turn that did read cache was counted as a cold one"
        );
    }

    /// A conversation split across files is one session, not two.
    #[test]
    fn a_resumed_session_merges_rather_than_doubling() {
        let ledger_of = |timestamp: &str, cwd: &str, input: i64| {
            aggregate(attribute(
                vec![turn("s1", timestamp, Some(cwd), input)],
                &[],
                KST,
            ))
        };
        let mut held = ledger_of("2026-08-19T01:00:00Z", "/w/a", 10);
        merge(&mut held, ledger_of("2026-08-19T05:00:00Z", "/w/b", 30));
        merge(&mut held, ledger_of("2026-08-18T23:00:00Z", "/w/a", 5));
        finalize(&mut held);

        assert_eq!(
            held.sessions.len(),
            1,
            "one conversation was reported as several"
        );
        let session = &held.sessions[0];
        assert_eq!(session.turn_count, 3);
        assert_eq!(session.total_input_tokens, 45);
        assert_eq!(
            session.first_timestamp, "2026-08-18T23:00:00Z",
            "the merge lost the earliest turn's start"
        );
        assert_eq!(session.last_timestamp, "2026-08-19T05:00:00Z");
        assert_eq!(
            session.location_breakdown.len(),
            2,
            "the two places were merged into one"
        );
        // Heaviest place first, counted in TOKENS rather than turns: `/w/b`
        // spent 40 across one turn where `/w/a` spent 35 across two.
        assert_eq!(session.location_breakdown[0].location_key, "cwd:/w/b");
        assert_eq!(session.location_breakdown[0].turn_count, 1);
        assert_eq!(session.location_breakdown[1].turn_count, 2);
        // One row per (day, model, project). All three turns are the same
        // local day — the 18th's 23:00Z is 08:00 on the 19th in Seoul — so
        // the two projects make two rows, not three.
        assert_eq!(held.daily_aggregates.len(), 2);
        assert!(
            held.daily_aggregates
                .iter()
                .all(|row| row.day == "2026-08-19"),
            "a stamp before UTC midnight was filed on the previous local day"
        );
    }

    /// Prices come from the table, and an unknown model has no price at all.
    #[test]
    fn a_model_is_priced_only_when_the_table_knows_it() {
        // One million of each counter makes the dollars readable as the rates.
        let million = |model: &str| estimate_cost_usd(Some(model), 1_000_000, 0, 0, 0);
        assert_eq!(million("claude-opus-5"), Some(5.0));
        assert_eq!(million("claude-fable-5"), Some(10.0));
        assert_eq!(million("claude-fable-5-1"), Some(10.0));
        // Fable 5.1 reads its cache at a quarter of Fable 5's rate — the one
        // number the two rows disagree on, so the point release must not fold
        // into its parent's row.
        let cached = |model: &str| estimate_cost_usd(Some(model), 0, 0, 1_000_000, 0);
        assert_eq!(cached("claude-fable-5-1"), Some(0.25));
        assert_eq!(cached("claude-fable-5"), Some(1.0));
        assert_eq!(million("claude-sonnet-5"), Some(3.0));
        assert_eq!(million("claude-haiku-4-5"), Some(1.0));
        // Version-bearing ids from real logs resolve to their family.
        assert_eq!(million("claude-opus-5-20260514"), Some(5.0));
        assert_eq!(million("anthropic/claude-sonnet-5"), Some(3.0));
        assert_eq!(million("claude-opus-4.8-thinking"), Some(5.0));
        // The legacy Opus 4 rate, and the boundary that separates it from 4-1.
        assert_eq!(million("claude-opus-4-20250101"), Some(15.0));
        assert_eq!(
            million("claude-opus-4-1"),
            Some(15.0),
            "opus-4-1 must not be read as a bare opus-4 point release"
        );
        assert_eq!(
            million("claude-opus-4-5"),
            Some(5.0),
            "the 4-5 generation bills at the current low Opus rate"
        );
        assert_eq!(
            estimate_cost_usd(Some("some-other-vendor-model"), 1_000_000, 0, 0, 0),
            None,
            "an unpriced model produced a number nobody can check"
        );
        assert_eq!(estimate_cost_usd(None, 1_000_000, 0, 0, 0), None);
    }

    /// Sonnet's long-context tier bills the excess at the higher rate.
    #[test]
    fn sonnet_bills_the_context_above_two_hundred_thousand_higher() {
        // 300k input: 200k at $3/M and 100k at $6/M.
        let cost = estimate_cost_usd(Some("claude-sonnet-4-6"), 300_000, 0, 0, 0).expect("priced");
        let expected = (200_000.0 * 3.0 + 100_000.0 * 6.0) / 1_000_000.0;
        assert!(
            (cost - expected).abs() < 1e-9,
            "the long-context tier was ignored: {cost} vs {expected}"
        );
        // Sonnet 5 bills its whole window flat, so no tier applies.
        let flat = estimate_cost_usd(Some("claude-sonnet-5"), 300_000, 0, 0, 0).expect("priced");
        assert!(
            (flat - 300_000.0 * 3.0 / 1_000_000.0).abs() < 1e-9,
            "Sonnet 5 was billed a long-context tier it does not have"
        );
    }

    /// A range counts today and the days before it, inclusive.
    #[test]
    fn a_range_reaches_back_including_today() {
        // 2026-08-19T00:30Z is already the 19th in Seoul (09:30 local).
        let now = epoch_ms_of_iso("2026-08-19T00:30:00Z").expect("stamp");
        assert_eq!(
            range_cutoff(Range::Week, now, KST).as_deref(),
            Some("2026-08-13"),
            "a seven-day range that starts eight days ago counts eight days"
        );
        assert_eq!(
            range_cutoff(Range::Month, now, KST).as_deref(),
            Some("2026-07-21")
        );
        assert_eq!(range_cutoff(Range::All, now, KST), None);
    }

    /// A ledger with one managed and one unmanaged project, on two days.
    fn two_project_ledger() -> Ledger {
        let managed = WorktreeRef {
            repo_id: "repo".to_string(),
            worktree_id: "wt".to_string(),
            path: "/w/repo".to_string(),
            display_name: "repo".to_string(),
        };
        aggregate(attribute(
            vec![
                SourceTurn {
                    cache_read_tokens: 900,
                    cache_write_tokens: 100,
                    ..turn("in", "2026-08-19T01:00:00Z", Some("/w/repo/src"), 100)
                },
                SourceTurn {
                    model: Some("claude-sonnet-5".to_string()),
                    ..turn("out", "2026-08-12T01:00:00Z", Some("/elsewhere/thing"), 50)
                },
            ],
            &[managed],
            KST,
        ))
    }

    /// The scope keeps or drops whole projects, and the range whole days.
    #[test]
    fn the_report_answers_for_one_scope_and_one_range() {
        let ledger = two_project_ledger();
        let now = epoch_ms_of_iso("2026-08-19T02:00:00Z").expect("stamp");

        let everything = report(&ledger, Scope::All, Range::All, now, KST);
        assert_eq!(everything.summary.sessions, 2);
        assert_eq!(everything.summary.input_tokens, 150);
        assert_eq!(everything.daily.len(), 2, "two days collapsed into one bar");
        assert_eq!(everything.project_breakdown.len(), 2);
        assert_eq!(everything.model_breakdown.len(), 2);

        // Scope drops the session that never ran in a managed checkout.
        let ours = report(&ledger, Scope::Zerocode, Range::All, now, KST);
        assert_eq!(
            ours.summary.sessions, 1,
            "an unmanaged session survived the worktrees-only scope"
        );
        assert_eq!(ours.summary.input_tokens, 100);
        assert_eq!(ours.project_breakdown.len(), 1);
        assert_eq!(ours.project_breakdown[0].label, "repo");
        assert_eq!(
            ours.project_breakdown[0].sessions, 1,
            "the kept project lost its session count"
        );

        // Range drops the older day, whatever the scope.
        let week = report(&ledger, Scope::All, Range::Week, now, KST);
        assert_eq!(
            week.summary.sessions, 1,
            "a session outside the range was counted"
        );
        assert_eq!(week.daily.len(), 1);
        assert_eq!(week.daily[0].day, "2026-08-19");

        // And an empty answer says so rather than showing zeros as data.
        let empty = report(&Ledger::default(), Scope::All, Range::All, now, KST);
        assert!(
            !empty.summary.has_any_data,
            "an empty ledger claimed to have data"
        );
        assert_eq!(empty.summary.cache_reuse_rate, None);
        assert_eq!(empty.summary.estimated_cost_usd, None);
    }

    /// The derived percentages and the headline names.
    #[test]
    fn the_summary_derives_reuse_cost_and_the_heaviest_names() {
        let ledger = two_project_ledger();
        let now = epoch_ms_of_iso("2026-08-19T02:00:00Z").expect("stamp");
        let ours = report(&ledger, Scope::Zerocode, Range::All, now, KST);

        // 900 cache read against 100 input: 900 / (100 + 900).
        assert_eq!(ours.summary.cache_reuse_rate, Some(0.9));
        assert_eq!(ours.summary.top_model.as_deref(), Some("claude-opus-5"));
        assert_eq!(ours.summary.top_project.as_deref(), Some("repo"));
        assert_eq!(
            ours.summary.zero_cache_read_turns, 0,
            "a turn that read cache was counted as a cold start"
        );

        // Opus 5: 100 in, 10 out, 900 cache read, 100 cache write.
        let expected = (100.0 * 5.0 + 10.0 * 25.0 + 900.0 * 0.5 + 100.0 * 6.25) / 1_000_000.0;
        let cost = ours
            .summary
            .estimated_cost_usd
            .expect("a priced model has a cost");
        assert!((cost - expected).abs() < 1e-12, "{cost} vs {expected}");

        // The by-model list prices itself the same way.
        assert_eq!(ours.model_breakdown.len(), 1);
        let model_cost = ours.model_breakdown[0]
            .estimated_cost_usd
            .expect("the model row carries its own cost");
        assert!((model_cost - expected).abs() < 1e-12);
        assert!(
            ours.project_breakdown[0].estimated_cost_usd.is_none(),
            "a project row claimed a price, but a project is not billed at one rate"
        );
    }

    /// The recent-sessions table totals only the places the scope kept.
    #[test]
    fn a_session_row_reports_the_places_the_scope_kept() {
        let managed = WorktreeRef {
            repo_id: "repo".to_string(),
            worktree_id: "wt".to_string(),
            path: "/w/repo".to_string(),
            display_name: "repo".to_string(),
        };
        // One session that worked in a managed checkout and then elsewhere.
        let ledger = aggregate(attribute(
            vec![
                turn("s", "2026-08-19T01:00:00Z", Some("/w/repo/src"), 100),
                turn("s", "2026-08-19T03:00:00Z", Some("/elsewhere"), 50),
            ],
            &[managed],
            KST,
        ));
        let now = epoch_ms_of_iso("2026-08-19T04:00:00Z").expect("stamp");

        let all = report(&ledger, Scope::All, Range::All, now, KST);
        assert_eq!(all.recent_sessions.len(), 1);
        assert_eq!(all.recent_sessions[0].input_tokens, 150);
        assert_eq!(
            all.recent_sessions[0].project_label, "Multiple locations",
            "a session that moved between places named only one of them"
        );
        assert_eq!(
            all.recent_sessions[0].duration_minutes, 120,
            "the session's span is not first-to-last"
        );
        assert_eq!(all.recent_sessions[0].branch.as_deref(), Some("main"));

        let ours = report(&ledger, Scope::Zerocode, Range::All, now, KST);
        assert_eq!(
            ours.recent_sessions[0].input_tokens, 100,
            "the worktrees-only row counted tokens spent outside them"
        );
        assert_eq!(ours.recent_sessions[0].project_label, "repo");
    }
}
