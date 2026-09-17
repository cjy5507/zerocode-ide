//! What Codex spent, read off the rollout files it already writes.
//!
//! The same question `claude_tokens` answers for the other vendor, and a
//! harder one to answer honestly. Codex writes a `token_count` event per turn
//! carrying BOTH a running session total and that turn's own figure, and the
//! obvious reading — add the totals up — is wrong by fifty. Measured on one
//! session of this machine's corpus: the final total is 18,905,900 tokens, the
//! turns sum to 19,476,166, and adding the running totals gives
//! **1,012,523,733**.
//!
//! The rule that avoids it is Orca's, ported whole into
//! [`zerocode_core::codex_delta`] where it can be tested apart from the file
//! walking. Two of its decisions are worth repeating at the call site because
//! this module would otherwise look like it is doing something odd:
//!
//! - the spend is the turn's own figure, never the gap between two totals,
//!   because compaction and resume rewrite the totals underneath;
//! - an event's identity carries no session id, because a fork copies the
//!   record byte-for-byte into a new file and rewrites the id.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use zerocode_core::civil;
use zerocode_core::codex_delta::{self, RawUsage};

use crate::token_scan;
use crate::usage_places::Places;

/// Where the vendor keeps its rollouts, under the person's home.
///
/// Both roots: `sessions` is the live tree and `archived_sessions` is what it
/// retires into, and a person asking where their tokens went means both.
const SESSION_DIRS: [&str; 2] = ["sessions", "archived_sessions"];

/// The home of the vendor's own directory.
const CODEX_DIR: &str = ".codex";

/// Rollout files, and only those.
///
/// `~/.codex` also holds `history.jsonl` — a different record with no token
/// events in it — so the walk is narrowed by name rather than by extension.
const ROLLOUT_SUFFIX: &str = ".jsonl";
const ROLLOUT_PREFIX: &str = "rollout-";

/// The substring a token event cannot avoid carrying, checked before the line
/// is parsed. A rollout is mostly tool calls and message text; on this
/// machine's largest session only 127 of 700-odd records are token events.
const TOKEN_MARK: &str = "\"token_count\"";

/// The substring a model announcement carries.
const CONTEXT_MARK: &str = "\"turn_context\"";

/// The substring the record that names a rollout cannot avoid carrying.
///
/// The same cheap rejection the other two marks buy: a rollout opens with one
/// `session_meta` and then never mentions it again, so every other line in a
/// 504 MB file is thrown away by a memchr rather than by `serde_json`.
const META_MARK: &str = "\"session_meta\"";

/// One reading's tokens, in the four buckets this vendor reports.
///
/// Two of them are SHARES of the other two and never added to them:
/// `cached_input` is the part of `input` that was served from cache, and
/// `reasoning_output` is the part of `output` the model spent thinking. The
/// scanner keeps them apart because they are priced apart and drawn apart; a
/// reader who adds all four gets a number the vendor never billed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct CodexTokens {
    pub(crate) input: u64,
    pub(crate) cached_input: u64,
    pub(crate) output: u64,
    pub(crate) reasoning_output: u64,
}

impl CodexTokens {
    fn add(&mut self, other: Self) {
        self.input += other.input;
        self.cached_input += other.cached_input;
        self.output += other.output;
        self.reasoning_output += other.reasoning_output;
    }
}

/// One day's share, for one model, in one place.
///
/// Three-part key, the same as the Claude scan's and for the same reason:
/// every figure the screen shows is a sum over a subset of these rows, so one
/// walk answers every range, scope, model and place the filters can ask for.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CodexDayRow {
    /// `YYYY-MM-DD`, in the CALLER's day.
    pub(crate) date: String,
    pub(crate) model: String,
    /// The place key ([`crate::usage_places::Place::key`]) — the directory the
    /// turn ran in, resolved to one of this window's workspaces where it is one.
    pub(crate) project: String,
    /// What to call that place, or `None` when only the window has a word.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    pub(crate) ours: bool,
    /// Turns that spent something.
    pub(crate) turns: u64,
    /// Turns that read NOTHING back from the cache — Codex's own answer to the
    /// Claude scan's `zero_cache_read`, judged on `cached_input` because that
    /// is the bucket this vendor reports a cache hit in.
    pub(crate) zero_cached_input: u64,
    #[serde(flatten)]
    pub(crate) tokens: CodexTokens,
    /// What the day's turns cost at API rates, summed turn by turn — or `None`
    /// when this row's model has no rate on file.
    ///
    /// An estimate, and labelled as one wherever it is shown: a subscription
    /// is not billed per token.
    pub(crate) cost_usd: Option<f64>,
}

/// One model's whole share, across every day.
///
/// The same numbers as [`CodexDayRow`] without the date, and folded HERE
/// rather than in the window: it is the same arithmetic as the day rollup, and
/// a second copy of it in JavaScript is a second place to get wrong the one
/// rule that matters — `cost_usd: None` means "no rate on file", not "free".
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CodexModelRow {
    pub(crate) model: String,
    pub(crate) turns: u64,
    pub(crate) zero_cached_input: u64,
    #[serde(flatten)]
    pub(crate) tokens: CodexTokens,
    pub(crate) cost_usd: Option<f64>,
}

/// One Codex conversation, as the rollouts remember it.
///
/// The sibling of the Claude scan's session row, in this vendor's words, and
/// with the same two totals rather than a list of places — see [`SessionRow`]
/// there for why. No branch: a rollout records the repository it opened in but
/// not the branch, so a column for it would be empty on every row.
///
/// [`SessionRow`]: crate::claude_tokens::SessionRow
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CodexSessionRow {
    pub(crate) session: String,
    pub(crate) first_at: i64,
    pub(crate) last_at: i64,
    /// The CALLER's day of `last_at` — what the range filter compares, so the
    /// sessions table and the chart cannot disagree about midnight.
    pub(crate) last_date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    pub(crate) places: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ours_label: Option<String>,
    pub(crate) ours_places: u32,
    pub(crate) ours: bool,
    pub(crate) turns: u64,
    pub(crate) ours_turns: u64,
    pub(crate) tokens: CodexTokens,
    pub(crate) ours_tokens: CodexTokens,
    pub(crate) cost_usd: Option<f64>,
    pub(crate) ours_cost_usd: Option<f64>,
}

/// What one pass over the rollouts found.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CodexScan {
    /// Per day, model and place, newest first.
    pub(crate) days: Vec<CodexDayRow>,
    /// Every conversation, most recently active first — up to
    /// [`crate::claude_tokens::SESSION_ROWS_MAX`].
    pub(crate) sessions: Vec<CodexSessionRow>,
    /// How many distinct conversations the walk found, before the cap.
    pub(crate) sessions_seen: u64,
    /// Per model, heaviest first — the same order the Claude scan uses, so the
    /// two tables read the same way.
    pub(crate) models: Vec<CodexModelRow>,
    pub(crate) files: u64,
    /// Turns that spent something, added up — the headline count, and the
    /// Codex answer to Claude's `messages`. Counted here rather than summed in
    /// the window: the window renders, the scanner counts.
    pub(crate) turns: u64,
    /// Token events read, before the duplicates are dropped.
    pub(crate) events: u64,
    /// Events that spent nothing — a repeat, or a stale re-report of numbers
    /// already counted. The gap between this and `events` is what the delta
    /// rule is FOR, so it is reported rather than hidden.
    pub(crate) folded: u64,
    /// Turns with no readable timestamp, left out of the rows rather than put
    /// on a guessed day.
    pub(crate) undated: u64,
    /// Every priced row added up.
    pub(crate) cost_usd: f64,
    /// Models with no rate on file, named rather than folded in as free.
    pub(crate) unpriced: Vec<String>,
}

/// The rollout roots under `home`.
pub(crate) fn session_roots(home: &Path) -> Vec<PathBuf> {
    SESSION_DIRS
        .iter()
        .map(|name| home.join(CODEX_DIR).join(name))
        .collect()
}

/// One usage object out of the event's `info`.
fn usage_at(info: &serde_json::Value, name: &str) -> Option<RawUsage> {
    codex_delta::normalize(info.get(name))
}

/// Read one rollout, in order, into `spent`.
///
/// In order because the delta rule is a walk: each event is judged against the
/// totals the previous one left. `seen` is shared across every file, which is
/// the whole reason a forked session does not count twice.
/// Everything one pass accumulates, in one place.
///
/// A walk over 4.4 GB carries six running answers, and handing them over one
/// by one makes a signature nobody can read. They belong together anyway: each
/// is a fact about the same pass.
#[derive(Default)]
struct Tally {
    /// Every event already counted, across every file — a fork copies a
    /// session's events into a new rollout and a per-file set would let the
    /// copy through.
    seen: HashSet<String>,
    /// Day rows, keyed by date, model and place.
    spent: HashMap<(String, String, String), CodexDayRow>,
    /// Conversations, keyed by the id the rollout announced.
    sessions: HashMap<String, CodexSessionBuild>,
    events: u64,
    folded: u64,
    undated: u64,
}

fn absorb_rollout(path: &Path, offset_minutes: i32, tally: &mut Tally, places: &mut Places) {
    // The model a turn ran under is announced by a `turn_context` record and
    // holds until the next one — the token event itself does not name it.
    let mut model = String::from("unknown");
    // Two more facts the token event does not carry either. The rollout opens
    // with a `session_meta` naming both, and a `turn_context` can move the
    // directory mid-conversation — the tokens belong to where the turn RAN.
    //
    // A rollout that never announced itself is named by its own file, so a
    // conversation without a `session_meta` is still one conversation rather
    // than being folded in with every other nameless one.
    let mut session = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("rollout")
        .to_string();
    let mut place = places.of(None);
    let mut previous: Option<RawUsage> = None;
    token_scan::for_each_line(path, |text| {
        let carries_token = text.contains(TOKEN_MARK);
        let carries_meta = text.contains(META_MARK);
        if !carries_token && !carries_meta && !text.contains(CONTEXT_MARK) {
            return;
        }
        let Ok(row) = serde_json::from_str::<serde_json::Value>(text) else {
            return;
        };
        let payload = row.get("payload").unwrap_or(&serde_json::Value::Null);
        let said = |held: &serde_json::Value, name: &str| {
            held.get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let kind = row.get("type").and_then(serde_json::Value::as_str);
        if kind == Some("session_meta") {
            if let Some(named) = said(payload, "id").or_else(|| said(payload, "session_id")) {
                session = named;
            }
            if let Some(cwd) = said(payload, "cwd") {
                place = places.of(Some(&cwd));
            }
            return;
        }
        if kind == Some("turn_context") {
            if let Some(named) = said(payload, "model") {
                model = named;
            }
            if let Some(cwd) = said(payload, "cwd") {
                place = places.of(Some(&cwd));
            }
            return;
        }
        if payload.get("type").and_then(serde_json::Value::as_str) != Some("token_count") {
            return;
        }
        let Some(info) = payload.get("info") else {
            return;
        };
        let total = usage_at(info, "total_token_usage");
        let last = usage_at(info, "last_token_usage");
        let at = row.get("timestamp").and_then(serde_json::Value::as_str);
        tally.events += 1;

        // Named without the session, so the copy a fork made meets the
        // original here rather than being counted beside it.
        let key = codex_delta::event_key(at.unwrap_or_default(), total, last);
        if !tally.seen.insert(key) {
            tally.folded += 1;
            return;
        }
        let Some(held) = codex_delta::resolve(total, last, previous) else {
            tally.folded += 1;
            return;
        };
        let delta = match held {
            codex_delta::Resolution::Spend { delta, next_totals } => {
                previous = next_totals;
                delta
            }
            codex_delta::Resolution::Baseline { next_totals } => {
                previous = Some(next_totals);
                return;
            }
        };
        let Some(at) = at.and_then(zerocode_core::civil::epoch_ms_of_iso) else {
            tally.undated += 1;
            return;
        };
        let tokens = CodexTokens {
            input: delta.input,
            cached_input: delta.cached_input,
            output: delta.output,
            reasoning_output: delta.reasoning_output,
        };
        let date = civil::iso_date_of(at, offset_minutes);
        let dollars = cost_usd(&model, delta.input, delta.cached_input, delta.output);
        let one = places.at(place);
        let row = tally
            .spent
            .entry((date.clone(), model.clone(), one.key.clone()))
            .or_insert_with(|| CodexDayRow {
                date,
                model: model.clone(),
                project: one.key.clone(),
                label: one.label.clone(),
                ours: one.ours,
                turns: 0,
                zero_cached_input: 0,
                tokens: CodexTokens::default(),
                cost_usd: None,
            });
        row.turns += 1;
        if delta.cached_input == 0 {
            row.zero_cached_input += 1;
        }
        row.tokens.add(tokens);
        // Priced HERE, one turn at a time, because the long-context surcharge
        // is a property of a single request — see `cost_usd`.
        if let Some(spent) = dollars {
            row.cost_usd = Some(row.cost_usd.unwrap_or(0.0) + spent);
        }

        let ours = places.at(place).ours;
        let standing = tally
            .sessions
            .entry(session.clone())
            .or_insert_with(|| CodexSessionBuild {
                first_at: at,
                last_at: at,
                model: model.clone(),
                turns: 0,
                ours_turns: 0,
                tokens: CodexTokens::default(),
                ours_tokens: CodexTokens::default(),
                spent: 0.0,
                priced: false,
                ours_spent: 0.0,
                ours_priced: false,
                places: HashMap::new(),
            });
        standing.first_at = standing.first_at.min(at);
        if at >= standing.last_at {
            standing.last_at = at;
            standing.model = model.clone();
        }
        standing.turns += 1;
        standing.tokens.add(tokens);
        if let Some(spent) = dollars {
            standing.spent += spent;
            standing.priced = true;
        }
        if ours {
            standing.ours_turns += 1;
            standing.ours_tokens.add(tokens);
            if let Some(spent) = dollars {
                standing.ours_spent += spent;
                standing.ours_priced = true;
            }
        }
        let visited = standing.places.entry(place).or_default();
        visited.weight += tokens.input + tokens.output;
        visited.ours = ours;
    });
    // A rollout whose every event folded is a FORK — the copy of a
    // conversation already counted somewhere else. `event_key` deliberately
    // leaves the session id out (a fork rewrites it), so the copy's turns fold
    // to nothing and the row above is never opened for it. That is why the
    // session entry lives INSIDE the spend branch: an entry made earlier would
    // stand a zero-turn conversation beside the real one.
}

/// What one place contributed to one Codex conversation.
#[derive(Default)]
struct CodexPlaceWeight {
    weight: u64,
    ours: bool,
}

/// One Codex conversation, while it is still being added up.
struct CodexSessionBuild {
    first_at: i64,
    last_at: i64,
    model: String,
    turns: u64,
    ours_turns: u64,
    tokens: CodexTokens,
    ours_tokens: CodexTokens,
    spent: f64,
    priced: bool,
    ours_spent: f64,
    ours_priced: bool,
    places: HashMap<u32, CodexPlaceWeight>,
}

/// Every conversation the walk gathered, most recently active first.
fn fold_sessions(
    built: HashMap<String, CodexSessionBuild>,
    offset_minutes: i32,
    places: &Places,
) -> Vec<CodexSessionRow> {
    let mut sessions: Vec<CodexSessionRow> = built
        .into_iter()
        .map(|(session, one)| {
            // The heaviest place names the row, and the heaviest of OURS names
            // it when the screen is showing only ours. Ties break on the place
            // key so two runs read alike.
            let heaviest = |mine: bool| {
                one.places
                    .iter()
                    .filter(|(_, weight)| !mine || weight.ours)
                    .max_by(|left, right| {
                        left.1
                            .weight
                            .cmp(&right.1.weight)
                            .then_with(|| places.at(*right.0).key.cmp(&places.at(*left.0).key))
                    })
                    .and_then(|(at, _)| places.at(*at).label.clone())
            };
            let ours_places = one.places.values().filter(|weight| weight.ours).count();
            CodexSessionRow {
                session,
                first_at: one.first_at,
                last_at: one.last_at,
                last_date: civil::iso_date_of(one.last_at, offset_minutes),
                model: Some(one.model),
                label: heaviest(false),
                places: u32::try_from(one.places.len()).unwrap_or(u32::MAX),
                ours_label: heaviest(true),
                ours_places: u32::try_from(ours_places).unwrap_or(u32::MAX),
                ours: one.ours_turns > 0,
                turns: one.turns,
                ours_turns: one.ours_turns,
                tokens: one.tokens,
                ours_tokens: one.ours_tokens,
                cost_usd: one.priced.then_some(one.spent),
                ours_cost_usd: one.ours_priced.then_some(one.ours_spent),
            }
        })
        .collect();
    sessions.sort_by(|left, right| {
        right
            .last_at
            .cmp(&left.last_at)
            .then_with(|| left.session.cmp(&right.session))
    });
    sessions
}

/// One pass over every rollout under `home`, rolled up in the caller's day.
/// The day rows folded by model.
///
/// The weight that orders them is `input + output` and NOT the sum of all four
/// figures: `cached_input` is a share of `input` and `reasoning_output` a share
/// of `output`, so adding them in would rank a cache-heavy model above a
/// larger one for tokens it never spent twice.
fn fold_by_model(days: &[CodexDayRow]) -> Vec<CodexModelRow> {
    let mut held: HashMap<&str, CodexModelRow> = HashMap::new();
    for row in days {
        let into = held
            .entry(row.model.as_str())
            .or_insert_with(|| CodexModelRow {
                model: row.model.clone(),
                turns: 0,
                zero_cached_input: 0,
                tokens: CodexTokens::default(),
                cost_usd: None,
            });
        into.turns += row.turns;
        into.zero_cached_input += row.zero_cached_input;
        into.tokens.add(row.tokens);
        if let Some(spent) = row.cost_usd {
            into.cost_usd = Some(into.cost_usd.unwrap_or(0.0) + spent);
        }
    }
    let mut models: Vec<CodexModelRow> = held.into_values().collect();
    models.sort_by(|left, right| {
        let weight = |row: &CodexModelRow| row.tokens.input + row.tokens.output;
        weight(right)
            .cmp(&weight(left))
            .then_with(|| left.model.cmp(&right.model))
    });
    models
}

pub(crate) fn scan(home: &Path, offset_minutes: i32, places: &mut Places) -> CodexScan {
    let mut files = Vec::new();
    for root in session_roots(home) {
        files.extend(
            token_scan::files_under(&root, ROLLOUT_SUFFIX)
                .into_iter()
                .filter(|path| {
                    path.file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|name| name.starts_with(ROLLOUT_PREFIX))
                }),
        );
    }
    // Across every file, not per file: a fork copies a session's events into a
    // new rollout, and a per-file set would let the copy through.
    let mut tally = Tally::default();
    for path in &files {
        absorb_rollout(path, offset_minutes, &mut tally, places);
    }
    let Tally {
        spent,
        sessions,
        events,
        folded,
        undated,
        ..
    } = tally;
    let mut days: Vec<CodexDayRow> = spent.into_values().collect();
    days.sort_by(|left, right| {
        right
            .date
            .cmp(&left.date)
            .then_with(|| left.model.cmp(&right.model))
            .then_with(|| left.project.cmp(&right.project))
    });
    let mut spent_usd = 0.0;
    let mut unpriced: Vec<String> = Vec::new();
    for row in &days {
        match row.cost_usd {
            Some(dollars) => spent_usd += dollars,
            None => {
                if !unpriced.contains(&row.model) {
                    unpriced.push(row.model.clone());
                }
            }
        }
    }
    unpriced.sort();
    let models = fold_by_model(&days);
    let sessions_seen = sessions.len() as u64;
    let mut sessions = fold_sessions(sessions, offset_minutes, places);
    // Newest first, so the cap only ever drops conversations older than the
    // last one carried — the same rule the Claude scan keeps.
    sessions.truncate(crate::claude_tokens::SESSION_ROWS_MAX);
    CodexScan {
        turns: days.iter().map(|row| row.turns).sum(),
        days,
        sessions,
        sessions_seen,
        models,
        files: files.len() as u64,
        events,
        folded,
        undated,
        cost_usd: spent_usd,
        unpriced,
    }
}

/* ---- what it cost ------------------------------------------------------- */

/// Where the vendor's long-context surcharge begins, in tokens of ONE request
/// (`resources/model-prices.json`). The unit matters more than the number —
/// see [`cost_usd`].
fn long_context_threshold() -> u64 {
    model_prices::openai_long_context_threshold()
}

/// One bucket's cost, with the surcharge applied above the threshold.
fn tier_cost(tokens: u64, base: f64, long: Option<f64>) -> f64 {
    let threshold = long_context_threshold();
    match long {
        Some(long) if tokens > threshold => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "token counts are far inside f64"
            )]
            let (under, over) = (threshold as f64, (tokens - threshold) as f64);
            under * base + over * long
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "token counts are far inside f64"
        )]
        _ => tokens as f64 * base,
    }
}

/// What ONE TURN cost, in dollars — or `None` for a model with no rate.
///
/// Two things here are deliberate.
///
/// Cached input is subtracted from input before either is priced. The vendor
/// reports cached tokens as a SHARE of input, not beside it, so billing both
/// at full price would charge the cheap half twice
/// (`codex-usage-cost-estimate.ts:30-35`).
///
/// And the surcharge is applied **per turn**, which is where this parts from
/// Orca. Orca prices a whole day's row at once
/// (`codex-usage-rollup-projections.ts:48`), so a day that spent more than
/// 272,000 tokens in total is billed as though every one of them sat in a
/// long-context request. The threshold is a property of ONE request, and a
/// turn is the closest thing to a request that a rollout records. Measured on
/// this machine's corpus the difference is not academic: 0.9% of turns are
/// genuinely over the line, and pricing by day rather than by turn takes the
/// bill from $6,752.72 to $12,966.15 — 92% more.
pub(crate) fn cost_usd(model: &str, input: u64, cached_input: u64, output: u64) -> Option<f64> {
    let rate = model_prices::openai_rate(model)?;
    let cached = cached_input.min(input);
    let uncached = input - cached;
    let long = rate.long;
    Some(
        (tier_cost(uncached, rate.input, long.map(|held| held.input))
            + tier_cost(
                cached,
                rate.cached_input,
                long.map(|held| held.cached_input),
            )
            + tier_cost(output, rate.output, long.map(|held| held.output)))
            / 1_000_000.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lookup that knows no workspaces — most fixtures here are about the
    /// delta rule, not about where a turn ran.
    fn nowhere() -> Places {
        Places::new([])
    }

    fn event(at: &str, total: (u64, u64), last: (u64, u64)) -> String {
        serde_json::json!({
            "timestamp": at,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": total.0, "output_tokens": total.1,
                        "cached_input_tokens": 0, "reasoning_output_tokens": 0,
                        "total_tokens": total.0 + total.1,
                    },
                    "last_token_usage": {
                        "input_tokens": last.0, "output_tokens": last.1,
                        "cached_input_tokens": 0, "reasoning_output_tokens": 0,
                        "total_tokens": last.0 + last.1,
                    },
                },
            },
        })
        .to_string()
    }

    fn context(model: &str) -> String {
        serde_json::json!({
            "timestamp": "2026-08-18T00:00:00.000Z",
            "type": "turn_context",
            "payload": { "model": model },
        })
        .to_string()
    }

    fn corpus(files: &[(&str, Vec<String>)]) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("tempdir");
        let day = home
            .path()
            .join(CODEX_DIR)
            .join("sessions")
            .join("2026")
            .join("08");
        std::fs::create_dir_all(&day).expect("mkdir");
        for (name, lines) in files {
            std::fs::write(day.join(name), lines.join("\n")).expect("write");
        }
        home
    }

    /// The record a rollout opens with, naming itself and where it opened.
    fn meta(id: &str, cwd: &str, branch: Option<&str>) -> String {
        let mut payload = serde_json::json!({ "id": id, "cwd": cwd });
        if let Some(branch) = branch {
            payload["git"] = serde_json::json!({ "branch": branch });
        }
        serde_json::json!({
            "timestamp": "2026-08-18T00:00:00.000Z",
            "type": "session_meta",
            "payload": payload,
        })
        .to_string()
    }

    /// A turn context that moves the working directory as well as the model.
    fn context_at(model: &str, cwd: &str) -> String {
        serde_json::json!({
            "timestamp": "2026-08-18T00:00:00.000Z",
            "type": "turn_context",
            "payload": { "model": model, "cwd": cwd },
        })
        .to_string()
    }

    /// A lookup that knows one workspace.
    fn known() -> Places {
        Places::new([(PathBuf::from("/work/repo"), "main".to_string())])
    }

    /// A rollout says who it is and where it opened, and the rows carry both.
    #[test]
    fn a_rollout_is_filed_under_the_workspace_it_opened_in() {
        let home = corpus(&[(
            "rollout-a.jsonl",
            vec![
                meta("s-one", "/work/repo/src", Some("wt/drain")),
                context("gpt-5.6-sol"),
                event("2026-08-18T01:00:00.000Z", (1000, 10), (1000, 10)),
                event("2026-08-18T02:00:00.000Z", (2000, 20), (1000, 10)),
            ],
        )]);
        let held = scan(home.path(), 0, &mut known());
        assert_eq!(held.days.len(), 1);
        assert_eq!(held.days[0].project, "worktree:/work/repo");
        assert_eq!(held.days[0].label.as_deref(), Some("main"));
        assert!(held.days[0].ours);
        // Neither turn read anything back from the cache.
        assert_eq!(held.days[0].zero_cached_input, 2);
        assert_eq!(held.sessions.len(), 1);
        let one = &held.sessions[0];
        assert_eq!(one.session, "s-one");
        assert_eq!(one.turns, 2);

        assert_eq!(one.last_date, "2026-08-18");
        assert!(one.first_at < one.last_at);
        assert_eq!(one.places, 1);
        assert_eq!(one.ours_places, 1);
        assert!(one.ours);
        assert_eq!(one.label.as_deref(), Some("main"));
    }

    /// A turn run from somewhere else belongs to where it RAN, not to where
    /// the rollout was opened — `turn_context` carries the directory too.
    #[test]
    fn a_turn_that_moved_is_filed_where_it_ran() {
        let home = corpus(&[(
            "rollout-a.jsonl",
            vec![
                meta("s-one", "/work/repo", None),
                context("gpt-5.6-sol"),
                event("2026-08-18T01:00:00.000Z", (1000, 0), (1000, 0)),
                context_at("gpt-5.6-sol", "/elsewhere/scratch"),
                event("2026-08-18T02:00:00.000Z", (2000, 0), (1000, 0)),
            ],
        )]);
        let held = scan(home.path(), 0, &mut known());
        let keys: Vec<&str> = held.days.iter().map(|row| row.project.as_str()).collect();
        assert!(keys.contains(&"worktree:/work/repo"));
        assert!(keys.contains(&"cwd:/elsewhere/scratch"));
        let one = &held.sessions[0];
        assert_eq!(one.places, 2, "the move was not remembered");
        assert_eq!(one.ours_places, 1);
        assert_eq!(one.turns, 2);
        assert_eq!(one.ours_turns, 1);
        assert_eq!(one.turns, 2);
    }

    /// A rollout that never announced itself is still one conversation.
    ///
    /// Named by its own file rather than dropped: the turns happened and they
    /// belong together, and folding every nameless rollout into one row would
    /// invent a conversation that never existed.
    #[test]
    fn a_rollout_without_a_name_is_named_by_its_file() {
        let home = corpus(&[(
            "rollout-a.jsonl",
            vec![
                context("gpt-5.6-sol"),
                event("2026-08-18T01:00:00.000Z", (1000, 0), (1000, 0)),
            ],
        )]);
        let held = scan(home.path(), 0, &mut nowhere());
        assert_eq!(held.turns, 1);
        assert_eq!(held.days[0].turns, 1);
        assert_eq!(held.sessions.len(), 1);
        assert_eq!(held.sessions[0].session, "rollout-a");
        assert_eq!(held.sessions_seen, 1);
        // With nothing naming a directory, the row is filed under the key the
        // place lookup keeps for exactly that.
        assert_eq!(held.days[0].project, "unscoped");
        assert!(!held.days[0].ours);
        assert!(!held.sessions[0].ours);
    }

    /// A fork whose every event folds opens no conversation of its own.
    ///
    /// `event_key` deliberately leaves the session id out, so a fork's copied
    /// events meet the original and fold to nothing. If a row were opened
    /// before that fold, the screen would stand a zero-turn conversation beside
    /// the real one.
    #[test]
    fn a_fork_that_spends_nothing_opens_no_conversation() {
        let lines = vec![
            meta("s-one", "/work/repo", None),
            context("gpt-5.6-sol"),
            event("2026-08-18T01:00:00.000Z", (1000, 0), (1000, 0)),
        ];
        let mut forked = lines.clone();
        forked[0] = meta("s-two", "/work/repo", None);
        let home = corpus(&[("rollout-a.jsonl", lines), ("rollout-b.jsonl", forked)]);
        let held = scan(home.path(), 0, &mut known());
        assert_eq!(held.turns, 1, "the fork was billed again");
        assert_eq!(
            held.sessions.len(),
            1,
            "the fork stood a conversation of its own with nothing in it"
        );
        assert_eq!(held.sessions[0].session, "s-one");
    }

    /// The running totals are a baseline, not a bill.
    ///
    /// This is the whole slice in one assertion: three turns whose totals climb
    /// to 6,000 spend 3,000 between them, and a reader that added the totals up
    /// would say 12,000.
    #[test]
    fn a_session_spends_its_turns_and_not_its_running_totals() {
        let home = corpus(&[(
            "rollout-a.jsonl",
            vec![
                context("gpt-5.6-sol"),
                event("2026-08-18T01:00:00.000Z", (1000, 0), (1000, 0)),
                event("2026-08-18T02:00:00.000Z", (2000, 0), (1000, 0)),
                event("2026-08-18T03:00:00.000Z", (6000, 0), (1000, 0)),
            ],
        )]);
        let held = scan(home.path(), 0, &mut nowhere());
        assert_eq!(held.events, 3);
        assert_eq!(held.days.len(), 1);
        let row = &held.days[0];
        assert_eq!(
            row.model, "gpt-5.6-sol",
            "the turn context did not reach the row"
        );
        assert_eq!(row.turns, 3);
        assert_eq!(
            row.tokens.input, 3000,
            "the running totals were billed instead of the turns"
        );
    }

    /// A forked session copies the records; the copy is not a second spend.
    #[test]
    fn a_forked_rollout_does_not_bill_the_same_turn_twice() {
        let shared = vec![
            context("gpt-5.6-sol"),
            event("2026-08-18T01:00:00.000Z", (1000, 100), (1000, 100)),
            event("2026-08-18T02:00:00.000Z", (2000, 200), (1000, 100)),
        ];
        let home = corpus(&[
            ("rollout-first.jsonl", shared.clone()),
            // The fork: byte-for-byte the same events in another file.
            ("rollout-fork.jsonl", shared),
        ]);
        let held = scan(home.path(), 0, &mut nowhere());
        assert_eq!(held.events, 4, "both files were read");
        assert_eq!(held.folded, 2, "the copy was not recognised");
        assert_eq!(held.days[0].tokens.input, 2000, "the fork was billed again");
        assert_eq!(held.days[0].turns, 2);
    }

    /// The day is the caller's, and a turn with no stamp is set aside.
    #[test]
    fn the_rows_fall_on_the_callers_day_and_undated_turns_stand_apart() {
        let home = corpus(&[(
            "rollout-a.jsonl",
            vec![
                context("gpt-5.6-sol"),
                // 00:30Z — the day before, five hours west.
                event("2026-08-18T00:30:00.000Z", (10, 1), (10, 1)),
            ],
        )]);
        assert_eq!(
            scan(home.path(), 0, &mut nowhere()).days[0].date,
            "2026-08-18"
        );
        assert_eq!(
            scan(home.path(), -5 * 60, &mut nowhere()).days[0].date,
            "2026-08-17"
        );

        let home = corpus(&[(
            "rollout-b.jsonl",
            vec![
                context("gpt-5.6-sol"),
                serde_json::json!({
                    "type": "event_msg",
                    "payload": { "type": "token_count", "info": {
                        "last_token_usage": { "input_tokens": 5, "output_tokens": 1 },
                    }},
                })
                .to_string(),
            ],
        )]);
        let held = scan(home.path(), 0, &mut nowhere());
        assert_eq!(held.events, 1);
        assert_eq!(held.undated, 1);
        assert!(held.days.is_empty(), "an undated turn was put on a day");
    }

    /// `history.jsonl` sits in the same tree and is not a rollout.
    #[test]
    fn only_a_rollout_is_read() {
        let home = corpus(&[(
            "rollout-a.jsonl",
            vec![
                context("gpt-5.6-sol"),
                event("2026-08-18T01:00:00.000Z", (7, 0), (7, 0)),
            ],
        )]);
        let stray = home
            .path()
            .join(CODEX_DIR)
            .join("sessions")
            .join("2026")
            .join("08");
        std::fs::write(
            stray.join("history.jsonl"),
            event("2026-08-18T01:00:00.000Z", (999_999, 0), (999_999, 0)),
        )
        .expect("write");
        let held = scan(home.path(), 0, &mut nowhere());
        assert_eq!(held.files, 1, "a file that is not a rollout was read");
        assert_eq!(held.days[0].tokens.input, 7);
    }

    /// The rates, the cached-input rule, and the model ids that wear an effort
    /// tier.
    #[test]
    fn a_turn_is_priced_by_its_model_with_cached_input_counted_once() {
        // Under the long-context line throughout, so these read the base rates
        // alone — a million tokens would be OVER it and priced by the test
        // below instead.
        const UNDER: u64 = 100_000;
        let at = |dollars: f64| Some(UNDER as f64 * dollars / 1e6);
        assert_eq!(cost_usd("gpt-5.6-sol", UNDER, 0, 0), at(5.0));
        assert_eq!(cost_usd("gpt-5.6-sol", 0, 0, UNDER), at(30.0));
        // All of it cached: a tenth. And NOT input + cache — cached is a share
        // of input, not a bucket beside it.
        assert_eq!(cost_usd("gpt-5.6-sol", UNDER, UNDER, 0), at(0.5));
        // Part cached.
        assert_eq!(
            cost_usd("gpt-5.6-sol", UNDER, 40_000, 0),
            Some((60_000.0 * 5.0 + 40_000.0 * 0.5) / 1e6)
        );
        // More cached than input cannot make the uncached share negative.
        assert_eq!(
            cost_usd("gpt-5.6-sol", 1000, 9999, 0),
            Some(1000.0 * 0.5 / 1e6)
        );

        // The effort tier is not a price. Both spellings, and stacked.
        for said in [
            "gpt-5.6-sol",
            "GPT-5.6-Sol",
            "gpt-5.6-sol-high",
            "gpt-5.6-sol(high)",
            "gpt-5.6-sol-xhigh-auto",
        ] {
            assert_eq!(cost_usd(said, UNDER, 0, 0), at(5.0), "{said} lost its rate");
        }
        // The bare alias routes to Sol — exactly, never as a prefix.
        assert_eq!(cost_usd("gpt-5.6", UNDER, 0, 0), at(5.0));
        assert_eq!(
            cost_usd("gpt-5.6-luna", UNDER, 0, 0),
            at(1.0),
            "a cheaper 5.6 was swallowed by the alias"
        );
        // A parenthesised something that is not a tier is an id we do not know.
        assert_eq!(cost_usd("gpt-5.6-sol(turbo)", UNDER, 0, 0), None);
        // And a model with no row is unknown, not free.
        for unknown in ["codex-auto-review", "unknown", "gpt-9", ""] {
            assert_eq!(cost_usd(unknown, UNDER, 0, 0), None, "{unknown} was priced");
        }
    }

    /// The long-context surcharge is a property of ONE request, so it is
    /// applied per turn.
    ///
    /// Orca prices a whole day's row at once, which bills every token of a busy
    /// day as though it sat in a long-context request. On this machine's corpus
    /// that is the difference between $6,752.72 and $12,966.15.
    #[test]
    fn the_long_context_surcharge_is_measured_against_one_turn() {
        // Under the line: the base rate throughout.
        assert_eq!(
            cost_usd("gpt-5.6-sol", 100_000, 0, 0),
            Some(100_000.0 * 5.0 / 1e6)
        );
        // Exactly on it is still base — the surcharge is for what goes OVER.
        assert_eq!(
            cost_usd("gpt-5.6-sol", long_context_threshold(), 0, 0),
            Some(long_context_threshold() as f64 * 5.0 / 1e6)
        );
        // Over it: the first 272,000 at base and the rest at the surcharge.
        let over = long_context_threshold() + 100_000;
        assert_eq!(
            cost_usd("gpt-5.6-sol", over, 0, 0),
            Some((long_context_threshold() as f64 * 5.0 + 100_000.0 * 10.0) / 1e6)
        );
        // A model with no surcharge is unmoved by the size.
        assert_eq!(
            cost_usd("gpt-5.3-codex", over, 0, 0),
            Some(over as f64 * 1.75 / 1e6)
        );

        // And it reaches the rows one turn at a time: two turns under the line
        // cost less than one turn of the same total tokens over it.
        let apart = corpus(&[(
            "rollout-apart.jsonl",
            vec![
                context("gpt-5.6-sol"),
                event("2026-08-18T01:00:00.000Z", (200_000, 0), (200_000, 0)),
                event("2026-08-18T02:00:00.000Z", (400_000, 0), (200_000, 0)),
            ],
        )]);
        let together = corpus(&[(
            "rollout-together.jsonl",
            vec![
                context("gpt-5.6-sol"),
                event("2026-08-18T01:00:00.000Z", (400_000, 0), (400_000, 0)),
            ],
        )]);
        let apart = scan(apart.path(), 0, &mut nowhere()).cost_usd;
        let together = scan(together.path(), 0, &mut nowhere()).cost_usd;
        assert!(
            apart < together,
            "the surcharge was measured against the day, not the turn: {apart} vs {together}"
        );
        // The tokens are the same either way.
        assert_eq!(apart, 400_000.0 * 5.0 / 1e6);
    }

    /// A model with no rate is named rather than counted as free.
    #[test]
    fn an_unpriced_model_is_named_in_the_answer() {
        let home = corpus(&[(
            "rollout-a.jsonl",
            vec![
                context("codex-auto-review"),
                event("2026-08-18T01:00:00.000Z", (1000, 0), (1000, 0)),
                context("gpt-5.6-sol"),
                event("2026-08-18T02:00:00.000Z", (2000, 0), (1000, 0)),
            ],
        )]);
        let held = scan(home.path(), 0, &mut nowhere());
        assert_eq!(held.unpriced, vec!["codex-auto-review".to_string()]);
        assert!(
            held.cost_usd > 0.0,
            "the priced half was lost with the unpriced one"
        );
        let unknown = held
            .days
            .iter()
            .find(|row| row.model == "codex-auto-review")
            .expect("the unpriced row went missing");
        assert_eq!(unknown.cost_usd, None, "an unpriced row was given a price");
        assert_eq!(
            unknown.tokens.input, 1000,
            "its TOKENS were dropped along with its price"
        );
    }

    /// A home with no Codex in it counts nothing rather than failing.
    #[test]
    fn a_machine_without_codex_counts_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        let held = scan(home.path(), 0, &mut nowhere());
        assert_eq!(held.files, 0);
        assert_eq!(held.events, 0);
        assert!(held.days.is_empty());
        assert!(held.models.is_empty(), "a model appeared out of nothing");
    }

    /// The per-model fold adds every day up, keeps "no rate" as no rate, and
    /// ranks by what was actually spent.
    #[test]
    fn the_models_are_folded_across_days_without_inventing_a_price() {
        let home = corpus(&[(
            "rollout-a.jsonl",
            vec![
                context("gpt-5.6-sol"),
                event("2026-08-18T01:00:00.000Z", (1000, 50), (100, 50)),
                event("2026-08-19T01:00:00.000Z", (3000, 90), (200, 40)),
                context("codex-auto-review"),
                event("2026-08-19T02:00:00.000Z", (9_000_000, 90), (9_000_000, 0)),
            ],
        )]);
        let held = scan(home.path(), 0, &mut nowhere());
        // Three day rows (two dates for one model, one for the other), two
        // model rows.
        assert_eq!(held.days.len(), 3);
        assert_eq!(held.models.len(), 2);

        // Heaviest first, and the weight is input+output — the unpriced model
        // spent nine million tokens, so it leads even though it costs nothing
        // we can name.
        assert_eq!(held.models[0].model, "codex-auto-review");
        assert_eq!(
            held.models[0].cost_usd, None,
            "a model with no rate on file was given one"
        );
        assert_eq!(held.models[0].turns, 1);

        let sol = &held.models[1];
        assert_eq!(sol.model, "gpt-5.6-sol");
        assert_eq!(sol.turns, 2, "the two days were not added together");
        assert_eq!(held.turns, 3, "the headline turn count lost a row");
        // The DELTAS, not the running totals the events also carry.
        assert_eq!(sol.tokens.input, 300);
        assert_eq!(sol.tokens.output, 90);
        // And the fold agrees with the day rows it came from, to the cent.
        let by_day: f64 = held
            .days
            .iter()
            .filter(|row| row.model == "gpt-5.6-sol")
            .filter_map(|row| row.cost_usd)
            .sum();
        assert!(
            (sol.cost_usd.expect("a priced model lost its price") - by_day).abs() < 1e-9,
            "the model rollup disagrees with the days under it"
        );
    }
}
