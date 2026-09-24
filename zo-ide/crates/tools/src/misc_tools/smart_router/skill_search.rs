//! The skill search seat: every installed skill put to a System One judgment
//! against the task a turn describes, and the best of them handed back as a
//! tool result.
//!
//! The question, the shards, the checks on a reply, the relevance floor and
//! the word match behind all of it belong to `runtime::skill_rank`. This file
//! owns only what running it needs — the setting, the door, the shards on the
//! wire at the same time, the memo and the ledger row — the same shape as the
//! rerank seat next door (`rerank_shadow.rs`), so a reader of one can read
//! the other.
//!
//! Putting the catalog to the judgment sends the name and description of every
//! skill this machine has installed. That is a different thing to consent to
//! than a task's text or the vault's summaries, so it has its own switch,
//! `smart.skillSearch`, and is off unless a person writes one of its other
//! words. Each request then goes through the Jev door (`jev_gate`): consent,
//! budget, withheld lines and caps.
//!
//! # Nothing gets worse for asking
//!
//! A search that the door refuses, that fails, that misses the wall, or whose
//! answers all sit under the floor hands back what the word match ranked, and
//! the row says `routeUse: fallback`. So the prompt's index can be removed
//! without a turn ever being left with no way to find a skill.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use api::{
    SystemOneCall, SystemOneClient, SystemOneConfig, SystemOneFailure, SystemOneRequest,
    SYSTEMONE_MODEL,
};
use runtime::skill_rank::{
    lexical_rank, rank, skill_candidates, skill_questions, skill_shards, skill_state,
    validate_skills, narrow_questions, narrow_state, read_narrow, read_wide,
    suggestion_note, wide_questions, wide_state, SkillCandidate, SkillDetail,
    SkillReading, SkillSuggestionSeat, SKILL_RUBRIC_VERSION,
};
use runtime::{ContentBlock, ConversationMessage, MessageRole, SkillIndexEntry};
use serde::{Deserialize, Serialize};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::{JevMode, ROUTE_USE_APPLIED, ROUTE_USE_FALLBACK, SKILLS};

use super::jev_gate::{self, JevDoor};
use super::probe_exec::{remember_bounded, task_fingerprint};
use super::settings::skill_search_mode_from;
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

/// The skill search's ledger file — the Jev use table's name for this seat.
pub const SKILL_SEARCH_FILE: &str = SKILLS.ledger;

/// Outcome of a row whose judgment answered and checked out.
///
/// The door's word, because the hedge rule's sample reads it out of this
/// ledger (`JevDoor::hedge_for`) and a reader that spelled it differently
/// from the writer would find no answers at all.
pub const SKILL_OUTCOME_ANSWERED: &str = zerocode_core::jev::door::ANSWERED_OUTCOME;

/// The wall one search waits — the number the tool call sits through, and the
/// latency line the judge holds a rising seat to. The use table's own, so the
/// stage that waits and the judge that reads the wait cannot disagree.
pub const SKILL_SEARCH_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::SKILL_SEARCH_APPLY_DEADLINE_MS);
const _: () = assert!(
    matches!(
        SKILLS.apply_deadline_ms,
        Some(zerocode_core::jev::SKILL_SEARCH_APPLY_DEADLINE_MS)
    ),
    "the seat's row names the wall its stage waits"
);

const FAIL_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";
const FAIL_SKILL_SEND: &str = "skill_request_not_sent";

/// The word a caller's answer carries when there was nothing to ask about —
/// no skill installed, or no task described. No row is written for it: a
/// search nobody could have made is not evidence about the seat.
const NOTHING_TO_ASK: &str = "nothing_to_ask";

/// Where a project's skill-search ledger lives.
#[must_use]
pub fn skill_search_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, SKILL_SEARCH_FILE)
}

/// One search's row: what was asked, what came back, and which reader the
/// tool result actually came from.
///
/// No task text and no descriptions. The task is a fingerprint, and the
/// skills are named by the name the catalog already shows the model — a
/// ranking is unreadable without names, and a name is not a person's words.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillSearchRow {
    /// Unix milliseconds when the row was made.
    pub at: u64,
    /// Fingerprint of the task the skills were judged against.
    pub task: u64,
    /// Fingerprint of the catalog, names and descriptions both, so a row is
    /// only ever compared with rows about the same catalog.
    pub catalog: u64,
    #[serde(rename = "rubricVersion")]
    pub rubric_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "rubricFingerprint")]
    pub rubric_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "gateScore")]
    pub gate_score: Option<f64>,
    /// [`SKILL_OUTCOME_ANSWERED`], a failure's ledger token, or the door's
    /// refusal token.
    pub outcome: String,
    /// How many skills were put to the judgment.
    pub candidates: usize,
    /// How many requests the catalog was cut into, and how many of them came
    /// back checked. A shard is an independent request over its own skills,
    /// so the two differ whenever one reply was refused and the rest stood.
    pub shards: usize,
    #[serde(rename = "shardsAnswered")]
    pub shards_answered: usize,
    /// Which reader the tool result came from: the judgment
    /// ([`ROUTE_USE_APPLIED`]), a recording mode's word, or the word match
    /// ([`ROUTE_USE_FALLBACK`]).
    #[serde(rename = "routeUse")]
    pub route_use: String,
    /// True when the ranking was recalled from this process's memo rather
    /// than asked; the timing fields then say nothing.
    #[serde(default)]
    pub cached: bool,
    #[serde(default, rename = "elapsedMs")]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub retries: u32,
    /// The model that answered, as the response named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "inputTokens")]
    pub input_tokens: Option<u64>,
    /// Requests this search sent: none when the door refused it or the memo
    /// answered, one per shard plus their retries when they left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<u32>,
    /// Lines the door withheld from what was sent.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "redactedLines")]
    pub redacted_lines: Option<u32>,
    /// Which of a reply's rules refused it, on a row whose `outcome` is
    /// `schema` — one word for seven rules says a reply was refused but not by
    /// what, and the ledger is where the cause has to be readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
    /// Which skill the rule broke on, by its place in the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "rejectedAt")]
    pub rejected_at: Option<usize>,
    /// The skills handed back, best first, and what each one read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chosen: Vec<Chosen>,
    /// How many skills the judgment rated under the table's relevance floor —
    /// the number that says whether a search that answered found anything.
    #[serde(default)]
    pub under_floor: usize,
}

/// One skill a search handed back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chosen {
    pub name: String,
    /// The reading on 0 to 1, so a row reads the same whatever scale a later
    /// rubric uses.
    pub reading: f64,
    pub confidence: f64,
}

impl From<&SkillReading> for Chosen {
    fn from(reading: &SkillReading) -> Self {
        Self {
            name: reading.name.clone(),
            reading: reading.normalised,
            confidence: reading.confidence,
        }
    }
}

/// A later row saying whether the turn went on to load one of the skills the
/// search named — this seat's `agreed` mark, and the only evidence its `auto`
/// can rise on (`zerocode_core::jev::promote`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillLabelRow {
    pub at: u64,
    /// The skill the turn actually loaded.
    pub loaded: String,
    /// Whether the search that preceded it had named that skill.
    pub agreed: bool,
    /// Where it sat in the search's ranking, when it was in it at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "baselineAgreed")]
    pub baseline_agreed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "baselineLoaded")]
    pub baseline_loaded: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "unusedLoad")]
    pub unused_load: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "baselineUnusedLoad")]
    pub baseline_unused_load: Option<bool>,
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

impl SkillSearchRow {
    fn new(key: MemoKey, candidates: usize, shards: usize, outcome: String) -> Self {
        Self {
            at: unix_millis(),
            task: key.task,
            catalog: key.catalog,
            rubric_version: key.rubric,
            rubric_fingerprint: None,
            gate_score: None,
            outcome,
            candidates,
            shards,
            shards_answered: 0,
            route_use: ROUTE_USE_FALLBACK.to_string(),
            cached: false,
            elapsed_ms: 0,
            retries: 0,
            model: None,
            input_tokens: None,
            requests: Some(0),
            redacted_lines: Some(0),
            rejected: None,
            rejected_at: None,
            chosen: Vec::new(),
            under_floor: 0,
        }
    }
}

/// A search's ranking as this process remembers it. The key carries the
/// rubric version and the requested model — the door's, pinned or not
/// ([`jev_gate::model_key`]) — so a ranking made under other words or by
/// another model is never recalled for this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MemoKey {
    task: u64,
    catalog: u64,
    rubric: u32,
    model: u64,
}

impl MemoKey {
    fn for_search(task: &str, candidates: &[SkillCandidate], model: u64) -> Self {
        let mut catalog = String::new();
        for candidate in candidates {
            catalog.push_str(&candidate.name);
            catalog.push('\u{1f}');
            catalog.push_str(&candidate.description);
            catalog.push('\u{1e}');
        }
        Self {
            task: task_fingerprint(task, ""),
            catalog: task_fingerprint("", &catalog),
            rubric: SKILL_RUBRIC_VERSION,
            model,
        }
    }
}

fn memo() -> &'static Mutex<HashMap<MemoKey, Vec<SkillReading>>> {
    static MEMO: OnceLock<Mutex<HashMap<MemoKey, Vec<SkillReading>>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What one search settled: the ranking, and which reader produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct Searched {
    /// The skills, best first. Empty when nothing reached the floor and the
    /// word match found no shared vocabulary either.
    pub ranked: Vec<SkillReading>,
    /// [`ROUTE_USE_APPLIED`] when the judgment's ranking is what came back,
    /// [`ROUTE_USE_FALLBACK`] when the word match's did, and a recording
    /// mode's own word when the seat was asked but does not act.
    pub route_use: String,
    /// What the judgment's row said, for a caller that wants to say why.
    pub outcome: String,
    /// The skills the JUDGMENT named, whatever the turn went on to read —
    /// `None` when no judgment answered.
    ///
    /// Kept apart from [`Self::ranked`] on purpose: under a recording mode
    /// the turn reads the word match's order, and a mark that said the turn
    /// agreed with *that* would be evidence about the fallback rather than
    /// about the seat. What promotes this seat is whether the JUDGMENT named
    /// the skill the turn loaded ([`note_search_answer`]).
    pub judged_names: Option<Vec<String>>,
}

impl Searched {
    /// Whether the ranking came from the judgment rather than from the word
    /// match behind it.
    #[must_use]
    pub fn judged(&self) -> bool {
        self.route_use == ROUTE_USE_APPLIED
    }
}

/// Rank `skills` against `task`, on the road this project's setting names.
///
/// Always answers: the word match stands behind every ending but one, so a
/// caller never has to decide what to do with a search that did not happen.
/// The row is appended before this returns, so a reader of the ledger sees
/// the search that a tool result came from.
#[must_use]
pub fn search(cwd: &Path, task: &str, skills: &[SkillIndexEntry]) -> Searched {
    let candidates = skill_candidates(skills);
    let fallback = |outcome: &str| Searched {
        ranked: lexical_rank(task, &candidates),
        route_use: ROUTE_USE_FALLBACK.to_string(),
        outcome: outcome.to_string(),
        judged_names: None,
    };
    if candidates.is_empty() || task.trim().is_empty() {
        return fallback(NOTHING_TO_ASK);
    }
    let Some(mode) = asking_mode(cwd) else {
        return fallback(JevMode::Off.key());
    };
    // Read once, here: the standing decides both whether the tool result is
    // the judgment's and whether a second request is worth buying, and two
    // readings of one ledger could answer those two questions differently.
    let acting = mode.applies_with(runtime::jev_seat_applies(cwd, &SKILLS));
    let (mut row, ranked) =
        api::sync_bridge::run_blocking(judge(cwd, task, &candidates, acting));
    let judged = row.outcome == SKILL_OUTCOME_ANSWERED;
    let judged_names = judged.then(|| {
        ranked
            .iter()
            .map(|reading| reading.name.clone())
            .collect::<Vec<_>>()
    });
    // A recording mode asked and wrote the row down; what the turn reads is
    // still the word match's ranking, exactly as it would be with the switch
    // off. That is what makes the two readable side by side.
    let searched = if judged && acting {
        Searched {
            ranked,
            route_use: ROUTE_USE_APPLIED.to_string(),
            outcome: row.outcome.clone(),
            judged_names,
        }
    } else {
        Searched {
            ranked: lexical_rank(task, &candidates),
            route_use: if judged {
                mode.key().to_string()
            } else {
                ROUTE_USE_FALLBACK.to_string()
            },
            outcome: row.outcome.clone(),
            judged_names,
        }
    };
    row.route_use.clone_from(&searched.route_use);
    let _ = append_shadow_row(&skill_search_path(cwd), &row, SHADOW_LEDGER_MAX_BYTES);
    searched
}

/// What the last search in this process handed back, per project — the names
/// a later load is judged against.
///
/// In memory and not on disk, deliberately: the mark this seat rises on is
/// whether THIS turn went on to load what THIS search named, and a name read
/// back from a ledger row could be a search somebody else's session made an
/// hour ago.
fn last_answer() -> &'static Mutex<HashMap<PathBuf, Vec<String>>> {
    static ANSWERED: OnceLock<Mutex<HashMap<PathBuf, Vec<String>>>> = OnceLock::new();
    ANSWERED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember what a search handed back, so a load that follows can be read as
/// agreeing with it or not.
pub fn note_search_answer(cwd: &Path, named: &[String]) {
    if let Ok(mut answered) = last_answer().lock() {
        answered.insert(cwd.to_path_buf(), named.to_vec());
    }
}

/// Write this seat's `agreed` mark: the turn loaded `loaded`, and the last
/// search either named it or did not.
///
/// It is a row of its own rather than a column on the search's row, because
/// the search's row is written before anybody knows what the turn will do —
/// and the judge reads every row that carries the mark
/// (`zerocode_core::jev::summary::AGREED`).
///
/// Nothing is written when no search has been made for this project in this
/// process: a load nobody asked a question before is not a judgment anyone
/// agreed or disagreed with.
pub fn note_loaded_skill(cwd: &Path, loaded: &str) {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if let Ok(mut pending) = turn_pending().lock() {
        if let Some(turn) = pending.get_mut(&canonical) {
            if turn.loaded.is_none() {
                turn.loaded = Some(loaded.to_string());
            }
            return;
        }
    }
    let Some(named) = last_answer()
        .lock()
        .ok()
        .and_then(|mut answered| answered.remove(cwd))
    else {
        return;
    };
    let row = label_row(loaded, &named);
    let _ = append_shadow_row(&skill_search_path(cwd), &row, SHADOW_LEDGER_MAX_BYTES);
}

/// The mark itself: whether the skill the turn loaded was one the search
/// named, and where in the ranking it sat.
fn label_row(loaded: &str, named: &[String]) -> SkillLabelRow {
    let rank = named.iter().position(|name| name == loaded);
    SkillLabelRow {
        at: unix_millis(),
        loaded: loaded.to_string(),
        agreed: rank.is_some() || (loaded.is_empty() && named.is_empty()),
        rank,
        baseline_agreed: None,
        baseline_loaded: None,
        unused_load: None,
        baseline_unused_load: None,
    }
}

/// The turn boundary's two-stage suggestion uses this same SKILLS door and
/// ledger. It is seated independently of the explicit `skill_search` tool so a
/// turn reaches the judgment even when the agent never searches.
#[derive(Debug)]
pub struct SkillSuggestionJudge {
    cwd: PathBuf,
}

impl SkillSuggestionJudge {
    #[must_use]
    pub fn at(cwd: &Path) -> Self {
        Self { cwd: cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()) }
    }
}

impl SkillSuggestionSeat for SkillSuggestionJudge {
    fn suggest(&self, request: String) -> futures_util::future::BoxFuture<'_, Option<String>> {
        Box::pin(suggest_at(self.cwd.clone(), request))
    }

    fn finish(&self, turn: &[ConversationMessage]) {
        finish_turn_suggestion(&self.cwd, turn);
    }
}

/// The next observed Skill load is the first-load label for this turn. A turn
/// with no load is closed by the runtime's turn-end hook. The entry is seated
/// before the judgment is asked, so a load early in the turn is its label
/// whether or not the judgment has landed yet; `judged` says whether it did,
/// and a turn that ends without one is not labelled — there is nothing to
/// compare the load with.
#[derive(Debug)]
struct PendingSuggestion {
    generation: u64,
    suggested: Option<String>,
    loaded: Option<String>,
    acting: bool,
    judged: bool,
}

static NEXT_SUGGESTION_GENERATION: AtomicU64 = AtomicU64::new(1);
static LATE_SUGGESTION_JUDGMENTS: AtomicU64 = AtomicU64::new(0);

fn turn_pending() -> &'static Mutex<HashMap<PathBuf, PendingSuggestion>> {
    static PENDING: OnceLock<Mutex<HashMap<PathBuf, PendingSuggestion>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn start_pending_suggestion(cwd: &Path, acting: bool) -> Option<u64> {
    let mut pending = turn_pending().lock().ok()?;
    let generation = NEXT_SUGGESTION_GENERATION.fetch_add(1, Ordering::Relaxed);
    pending.insert(cwd.to_path_buf(), PendingSuggestion {
        generation,
        suggested: None,
        loaded: None,
        acting,
        judged: false,
    });
    Some(generation)
}

fn mark_pending_suggestion_judged(cwd: &Path, generation: u64, suggested: Option<&str>) {
    if let Ok(mut pending) = turn_pending().lock() {
        match pending.get_mut(cwd) {
            Some(turn) if turn.generation == generation => {
                turn.suggested = suggested.map(str::to_string);
                turn.judged = true;
            }
            _ => {
                LATE_SUGGESTION_JUDGMENTS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

fn finish_turn_suggestion(cwd: &Path, turn: &[ConversationMessage]) {
    if let Ok(mut answered) = last_answer().lock() {
        answered.remove(cwd);
    }
    let pending = turn_pending().lock().ok().and_then(|mut rows| rows.remove(cwd));
    if let Some(row) = pending.and_then(|pending| judged_label(pending, turn)) {
        let _ = append_shadow_row(&skill_search_path(cwd), &row, SHADOW_LEDGER_MAX_BYTES);
    }
}

/// The turn's label, when its judgment landed while the turn was open; a
/// turn whose entry was never marked judged has nothing to compare its load
/// with, and gets no row.
fn judged_label(pending: PendingSuggestion, turn: &[ConversationMessage]) -> Option<SkillLabelRow> {
    pending.judged.then(|| label_turn(pending, turn))
}

fn label_turn(pending: PendingSuggestion, turn: &[ConversationMessage]) -> SkillLabelRow {
    let loaded = pending.loaded.as_deref().unwrap_or_default();
    let mut row = label_row(loaded, &pending.suggested.into_iter().collect::<Vec<_>>());
    row.unused_load = (!loaded.is_empty()).then(|| skill_used_after_load(turn, loaded))
        .flatten().map(|used| !used);
    if !pending.acting {
        row.baseline_loaded = pending.loaded;
        row.baseline_unused_load = row.unused_load;
    }
    row
}

/// A deliberately narrow proxy: after a successful first skill load, the
/// agent used another tool or quoted a substantial line from that skill's
/// returned body. Silence is an unused-load observation, not a gold label.
const SKILL_QUOTE_MIN_CHARS: usize = 24;
const SKILL_QUOTE_MAX_CHARS: usize = 160;
const SKILL_QUOTE_LINE_CAP: usize = 64;

fn skill_used_after_load(turn: &[ConversationMessage], loaded: &str) -> Option<bool> {
    let mut load_id: Option<&str> = None;
    let mut quotes = Vec::new();
    for message in turn {
        for block in &message.blocks {
            match block {
                ContentBlock::ToolUse { id, name, input } if load_id.is_none()
                    && skill_call_loaded(name, input, loaded) =>
                {
                    load_id = Some(id);
                }
                ContentBlock::ToolUse { name, .. } if load_id.is_some()
                    && !matches!(name.as_str(), "Skill" | "skill_load" | "skill_search") =>
                {
                    return Some(true);
                }
                ContentBlock::ToolResult { tool_use_id, output, .. }
                    if load_id == Some(tool_use_id.as_str()) =>
                {
                    quotes = skill_quote_lines(output);
                }
                ContentBlock::Text { text } if load_id.is_some()
                    && message.role == MessageRole::Assistant
                    && quotes.iter().any(|quote| text.contains(quote)) =>
                {
                    return Some(true);
                }
                _ => {}
            }
        }
    }
    load_id.map(|_| false)
}

fn skill_call_loaded(name: &str, input: &str, loaded: &str) -> bool {
    if name == "skill_search" {
        return true;
    }
    if !matches!(name, "Skill" | "skill_load") {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(input).ok().is_some_and(|value| {
        value.get("skill").or_else(|| value.get("name"))
            .and_then(serde_json::Value::as_str) == Some(loaded)
    })
}

fn skill_quote_lines(output: &str) -> Vec<String> {
    fn prompts<'a>(value: &'a serde_json::Value, found: &mut Vec<&'a str>) {
        match value {
            serde_json::Value::Object(fields) => {
                if let Some(prompt) = fields.get("prompt").and_then(serde_json::Value::as_str) {
                    found.push(prompt);
                } else {
                    for field in fields.values() {
                        prompts(field, found);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    prompts(item, found);
                }
            }
            _ => {}
        }
    }
    let parsed = serde_json::from_str::<serde_json::Value>(output).ok();
    let mut bodies = Vec::new();
    if let Some(value) = &parsed {
        prompts(value, &mut bodies);
    }
    bodies.into_iter().flat_map(str::lines)
        .map(str::trim)
        .filter(|line| (SKILL_QUOTE_MIN_CHARS..=SKILL_QUOTE_MAX_CHARS).contains(&line.chars().count()))
        .take(SKILL_QUOTE_LINE_CAP)
        .map(str::to_string)
        .collect()
}

/// The seat's line is shown only when it acts, so only then does the turn
/// wait for it: a recording turn hands the judgment to the runtime's worker
/// threads and goes on, the way the patch-review seat's `detach` does. The
/// mode is read before the skills are discovered, so a seat that is off costs
/// a turn no directory walk.
async fn suggest_at(cwd: PathBuf, task: String) -> Option<String> {
    if let Ok(mut answered) = last_answer().lock() {
        answered.remove(&cwd);
    }
    // A judgment that landed after its own turn had ended left no label
    // behind, and it must not label this turn either.
    if let Ok(mut pending) = turn_pending().lock() {
        pending.remove(&cwd);
    }
    if task.trim().is_empty() {
        return None;
    }
    let mode = asking_mode(&cwd)?;
    let acting = mode.applies_with(runtime::jev_seat_applies(&cwd, &SKILLS));
    let skills = runtime::discover_skills(&cwd);
    let candidates = skill_candidates(&skills);
    if candidates.is_empty() {
        return acting.then(|| suggestion_note(None));
    }
    if candidates.len() > zerocode_core::jev::SKILL_SUGGESTION_CATALOG_CAP {
        return None;
    }
    let generation = start_pending_suggestion(&cwd, acting);
    let judged = judge_suggestion(cwd, task, skills, candidates, mode, acting, generation);
    if acting {
        judged.await
    } else {
        super::patch_review::detach(async move {
            drop(judged.await);
        });
        None
    }
}

/// Ask the two stages, write the row, and mark the turn's pending entry
/// judged — when the turn is still open; a turn that has already ended took
/// its entry with it, and a judgment that lands after it labels nothing.
async fn judge_suggestion(
    cwd: PathBuf,
    task: String,
    skills: Vec<SkillIndexEntry>,
    candidates: Vec<SkillCandidate>,
    mode: JevMode,
    acting: bool,
    generation: Option<u64>,
) -> Option<String> {
    let opened = cwd.clone();
    let door = tokio::task::spawn_blocking(move || JevDoor::open(&opened)).await.ok()?;
    let mut key = MemoKey::for_search(&task, &candidates, door.model_key());
    key.rubric = zerocode_core::jev::questions::SKILL_SUGGESTION_RUBRIC_VERSION;
    let mut row = SkillSearchRow::new(key, candidates.len(), 1, String::new());
    row.rubric_fingerprint = Some(
        zerocode_core::jev::questions::skill_suggestion_rubric_fingerprint(),
    );
    let Some(client) = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client) else {
        row.outcome = Refused::NoKey.token().into();
        telemetry::attest_declined(telemetry::HarnessFeature::SkillSearch, Refused::NoKey.token());
        let _ = append_shadow_row(&skill_search_path(&cwd), &row, SHADOW_LEDGER_MAX_BYTES);
        return None;
    };
    let started = Instant::now();
    let wide = ask_wide(&door, &client, &task, &candidates, acting, &mut row).await;
    let Some(wide) = wide else {
        let _ = append_shadow_row(&skill_search_path(&cwd), &row, SHADOW_LEDGER_MAX_BYTES);
        return None;
    };
    row.gate_score = Some(wide.gate);
    let winner = if wide.shortlist.is_empty() {
        None
    } else {
        match ask_narrow(&door, &client, &task, &skills, &candidates, &wide.shortlist,
            acting, started, &mut row).await {
            Ok(winner) => winner,
            Err(reason) => {
                row.outcome = SystemOneFailure::Schema.ledger_token();
                row.rejected = Some(reason);
                let _ = append_shadow_row(&skill_search_path(&cwd), &row, SHADOW_LEDGER_MAX_BYTES);
                return None;
            }
        }
    };
    row.outcome = SKILL_OUTCOME_ANSWERED.into();
    telemetry::attest_fired(telemetry::HarnessFeature::SkillSearch);
    row.chosen = winner.iter().map(Chosen::from).collect();
    row.route_use = if acting { ROUTE_USE_APPLIED.into() } else { mode.key().into() };
    let _ = append_shadow_row(&skill_search_path(&cwd), &row, SHADOW_LEDGER_MAX_BYTES);
    // The absence of a skill is a prediction too. It stays until this turn
    // ends so a no-load turn contributes a negative label.
    if let Some(generation) = generation {
        mark_pending_suggestion_judged(&cwd, generation, winner.as_ref().map(|choice| choice.name.as_str()));
    }
    acting.then(|| suggestion_note(winner.as_ref().map(|choice| choice.name.as_str())))
}

async fn ask_wide(
    door: &JevDoor,
    client: &SystemOneClient,
    task: &str,
    candidates: &[SkillCandidate],
    acting: bool,
    row: &mut SkillSearchRow,
) -> Option<runtime::skill_rank::WideReading> {
    let state = wide_state(task, candidates);
    let questions = wide_questions(candidates);
    let (call, withheld) = match send_stage(
        door, client, &state, &questions, acting, SKILL_SEARCH_DEADLINE,
    ).await {
        Ok(value) => value,
        Err(reason) => {
            row.outcome = reason;
            return None;
        }
    };
    row.requests = Some(call.requests);
    row.retries = call.retries;
    row.elapsed_ms = jev_gate::millis(call.elapsed);
    row.redacted_lines = Some(withheld);
    let response = match call.outcome {
        Ok(response) => response,
        Err(failure) => {
            row.outcome = failure.ledger_token();
            return None;
        }
    };
    row.model = Some(response.model.clone());
    row.input_tokens = Some(response.usage.input_tokens);
    match read_wide(&response, candidates) {
        Ok(wide) => {
            row.shards_answered = 1;
            Some(wide)
        }
        Err(reason) => {
            row.outcome = SystemOneFailure::Schema.ledger_token();
            row.rejected = Some(reason.into());
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn ask_narrow(
    door: &JevDoor,
    client: &SystemOneClient,
    task: &str,
    skills: &[SkillIndexEntry],
    candidates: &[SkillCandidate],
    shortlist: &[usize],
    acting: bool,
    started: Instant,
    row: &mut SkillSearchRow,
) -> Result<Option<SkillReading>, String> {
    let details = shortlist.iter().map(|&position| SkillDetail {
        position,
        excerpt: skills.get(position).and_then(|skill| std::fs::read_to_string(&skill.path).ok())
            .unwrap_or_default(),
    }).collect::<Vec<_>>();
    let state = narrow_state(task, candidates, &details);
    let questions = narrow_questions(candidates, &details);
    row.shards = 2;
    let remaining = SKILL_SEARCH_DEADLINE.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err("deadline".into());
    }
    let (call, withheld) = send_stage(door, client, &state, &questions, acting, remaining).await?;
    row.requests = Some(row.requests.unwrap_or(0).saturating_add(call.requests));
    row.retries = row.retries.saturating_add(call.retries);
    row.elapsed_ms = row.elapsed_ms.saturating_add(jev_gate::millis(call.elapsed));
    row.redacted_lines = Some(row.redacted_lines.unwrap_or(0).saturating_add(withheld));
    let response = call.outcome.map_err(SystemOneFailure::ledger_token)?;
    row.input_tokens = Some(row.input_tokens.unwrap_or(0).saturating_add(response.usage.input_tokens));
    let winner = read_narrow(&response, candidates, &details).map_err(str::to_string)?;
    row.shards_answered = 2;
    Ok(winner)
}

async fn send_stage(
    door: &JevDoor,
    client: &SystemOneClient,
    state: &serde_json::Value,
    questions: &std::collections::BTreeMap<String, api::SystemOneQuestion>,
    acting: bool,
    deadline: Duration,
) -> Result<(SystemOneCall, u32), String> {
    let request = SystemOneRequest { state, model: SYSTEMONE_MODEL, questions };
    let body = jev_gate::body_of(&request)
        .ok_or_else(|| SystemOneFailure::InvalidRequest.ledger_token())?;
    let cleared = door.pass(&SKILLS, true, body).map_err(|refusal| refusal.token().to_string())?;
    let withheld = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    let hedge = door.hedge_now(&SKILLS, deadline, acting);
    Ok((jev_gate::send(client, cleared, deadline, hedge).await, withheld))
}

/// The mode this search is to be judged under, or `None` when it is not to be
/// judged at all: an ablation holding it out, an unreadable setting, or a
/// mode that asks nothing.
fn asking_mode(cwd: &Path) -> Option<JevMode> {
    if telemetry::attest_ablated(telemetry::HarnessFeature::SkillSearch) {
        return None;
    }
    let Some(mode) = skill_search_mode_from(&runtime::ConfigLoader::default_for(cwd)) else {
        telemetry::attest_failed(telemetry::HarnessFeature::SkillSearch, FAIL_SETTINGS_UNAVAILABLE);
        return None;
    };
    if !mode.asks() {
        telemetry::attest_declined(telemetry::HarnessFeature::SkillSearch, mode.key());
        return None;
    }
    Some(mode)
}

/// One search's row and its ranking: recalled from the memo, refused at the
/// door, or asked in shards and checked.
async fn judge(
    cwd: &Path,
    task: &str,
    candidates: &[SkillCandidate],
    acting: bool,
) -> (SkillSearchRow, Vec<SkillReading>) {
    let shards = skill_shards(candidates);
    // The door before the memo: what a ranking is remembered under is the
    // model the door asks for, and only the person's settings say which.
    let cwd_owned = cwd.to_path_buf();
    let Ok(door) = tokio::task::spawn_blocking(move || JevDoor::open(&cwd_owned)).await else {
        telemetry::attest_failed(telemetry::HarnessFeature::SkillSearch, FAIL_SETTINGS_UNAVAILABLE);
        let row = SkillSearchRow::new(
            MemoKey::for_search(task, candidates, jev_gate::model_key(SYSTEMONE_MODEL)),
            candidates.len(),
            shards.len(),
            FAIL_SETTINGS_UNAVAILABLE.to_string(),
        );
        return (row, Vec::new());
    };
    let key = MemoKey::for_search(task, candidates, door.model_key());
    if let Some(remembered) = memo().lock().ok().and_then(|memo| memo.get(&key).cloned()) {
        telemetry::attest_fired(telemetry::HarnessFeature::SkillSearch);
        let mut row = SkillSearchRow::new(
            key,
            candidates.len(),
            shards.len(),
            SKILL_OUTCOME_ANSWERED.to_string(),
        );
        row.cached = true;
        row.shards_answered = shards.len();
        row.chosen = remembered.iter().map(Chosen::from).collect();
        return (row, remembered);
    }
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    // Every shard at once: they are independent requests over disjoint
    // skills, and the caller waits for the slowest of them either way — which
    // is what makes an EVEN split worth having (`jev::shard`).
    let asked = shards
        .iter()
        .map(|shard| ask_one(&door, client.as_ref(), task, shard, acting));
    let answers = futures_util::future::join_all(asked).await;
    fold(key, candidates, &shards, answers)
}

/// What one shard's request came back with.
struct Shard {
    readings: Result<Vec<SkillReading>, Refusal>,
    call: Option<SystemOneCall>,
    withheld: u32,
}

/// Why a shard produced no readings, in the words a ledger row keeps.
struct Refusal {
    outcome: String,
    rejected: Option<String>,
    rejected_at: Option<usize>,
}

/// Ask one shard, through the door.
async fn ask_one(
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    task: &str,
    shard: &[SkillCandidate],
    acting: bool,
) -> Shard {
    let refused = |outcome: String| Shard {
        readings: Err(Refusal { outcome, rejected: None, rejected_at: None }),
        call: None,
        withheld: 0,
    };
    let state = skill_state(task, shard);
    let questions = skill_questions(shard);
    let Some(client) = client else {
        telemetry::attest_declined(telemetry::HarnessFeature::SkillSearch, Refused::NoKey.token());
        return refused(Refused::NoKey.token().to_string());
    };
    let (call, withheld) = match send_stage(door, client, &state, &questions, acting, SKILL_SEARCH_DEADLINE).await {
        Ok(sent) => sent,
        Err(reason) => {
            telemetry::attest_declined(telemetry::HarnessFeature::SkillSearch, FAIL_SKILL_SEND);
            return refused(reason);
        }
    };
    let readings = match &call.outcome {
        Ok(response) => validate_skills(shard, response).map_err(|refusal| Refusal {
            outcome: SystemOneFailure::Schema.ledger_token(),
            rejected: Some(refusal.rule().to_string()),
            rejected_at: refusal.position(),
        }),
        Err(failure) => Err(Refusal {
            outcome: failure.ledger_token(),
            rejected: None,
            rejected_at: None,
        }),
    };
    Shard { readings, call: Some(call), withheld }
}

/// Fold every shard's answer into one row and one ranking.
///
/// A shard that was refused takes its skills out of the ranking and nothing
/// else: the others asked about different skills and their answers stand. The
/// row's outcome is the search's — answered when anything was, and the first
/// refusal's word when nothing was.
fn fold(
    key: MemoKey,
    candidates: &[SkillCandidate],
    shards: &[&[SkillCandidate]],
    answers: Vec<Shard>,
) -> (SkillSearchRow, Vec<SkillReading>) {
    let mut row = SkillSearchRow::new(key, candidates.len(), shards.len(), String::new());
    let mut readings: Vec<SkillReading> = Vec::new();
    let mut first_refusal: Option<Refusal> = None;
    let (mut requests, mut withheld, mut input_tokens, mut elapsed, mut retries) = (0, 0, 0, 0, 0);
    for answer in answers {
        withheld += answer.withheld;
        if let Some(call) = &answer.call {
            requests += call.requests;
            retries = retries.max(call.retries);
            // The slowest shard is what the caller waited: they left together.
            elapsed = elapsed.max(jev_gate::millis(call.elapsed));
            if let Ok(response) = &call.outcome {
                input_tokens += response.usage.input_tokens;
                row.model.get_or_insert_with(|| response.model.clone());
            }
        }
        match answer.readings {
            Ok(read) => {
                row.shards_answered += 1;
                readings.extend(read);
            }
            Err(refusal) => {
                if first_refusal.is_none() {
                    first_refusal = Some(refusal);
                }
            }
        }
    }
    row.requests = Some(requests);
    row.redacted_lines = Some(withheld);
    row.input_tokens = (input_tokens > 0).then_some(input_tokens);
    row.elapsed_ms = elapsed;
    row.retries = retries;
    if row.shards_answered > 0 {
        telemetry::attest_fired(telemetry::HarnessFeature::SkillSearch);
        row.outcome = SKILL_OUTCOME_ANSWERED.to_string();
        let read = readings.len();
        let ranked = rank(readings);
        row.under_floor = read - ranked.len();
        row.chosen = ranked.iter().map(Chosen::from).collect();
        if let Ok(mut memo) = memo().lock() {
            remember_bounded(&mut memo, vec![(key, ranked.clone())]);
        }
        return (row, ranked);
    }
    let refusal = first_refusal.unwrap_or(Refusal {
        outcome: SystemOneFailure::NoKey.ledger_token(),
        rejected: None,
        rejected_at: None,
    });
    row.outcome = refusal.outcome;
    row.rejected = refusal.rejected;
    row.rejected_at = refusal.rejected_at;
    (row, Vec::new())
}

#[cfg(test)]
mod tests;
