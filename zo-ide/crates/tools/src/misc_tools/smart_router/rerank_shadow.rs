//! The memory rerank: every recall a turn performs is put to a System One
//! judgment, and what that judgment would have reordered — and what it would
//! have left out altogether — is written beside what recall chose. Under
//! `shadow` and `auto` that is all that happens and the turn reads exactly what
//! it would have read with the switch off; under `on` the judgment's answer —
//! after the vault's graph has had its say — is what the turn reads.
//!
//! The question, the checks on the reply, the rule that the vault's graph
//! outranks the judgment, the one ground on which a note is left out, and the
//! fold that accounts for every note recall admitted before anything changes
//! all belong to `runtime::memory::rerank`. This file owns only what running it
//! needs: the seat beside recall ([`RerankShadow`]), the
//! setting that says which road a recall takes, the call, the memo, and the
//! ledger row — the same shape as the routing shadow next door
//! (`decision_shadow.rs`), so a reader of one can read the other.
//!
//! Putting a recall's notes to the judgment sends the vault's own summaries off
//! the machine. That is a different thing to consent to than the routing
//! shadow's task text, so it has its own switch, `smart.rerankShadow`, and is
//! off unless a person writes one of its other words. Each request then goes
//! through the Jev door (`jev_gate`), as the routing shadow's do: consent,
//! budget, withheld lines and caps.
//!
//! # Nothing gets worse for asking
//!
//! The apply road can only ever hand back recall's own order, or an order whose
//! two lists account for every note recall admitted exactly once. A judgment
//! that misses the wall, that the door refuses, that fails its checks, or whose
//! lists cannot be checked that far leaves recall's order standing, and the row
//! says `applied: false` either way — so the ledger can be read back for how
//! often the switch actually changed anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{
    SystemOneCall, SystemOneClient, SystemOneConfig, SystemOneFailure, SystemOneRequest,
    SYSTEMONE_MODEL,
};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::RECALL;
use runtime::memory::rerank::{
    apply_order, compare, rerank_candidates, rerank_questions, rerank_state, validate_rerank,
    RerankComparison, RerankReading, RERANK_RUBRIC_VERSION,
};
use core_types::{ContentBlock, ConversationMessage, MessageRole};
use runtime::{MemoryHit, RecallSeat};
use serde::{Deserialize, Serialize};

use super::jev_gate::{self, JevDoor};
use super::probe_exec::{remember_bounded, task_fingerprint, PROBE_TIMEOUT};
use super::settings::rerank_shadow_mode_from;
use super::shadow_ledger::{
    append_shadow_row, judge_seat_ledger, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES,
};
use crate::misc_tools::agent_tools::shared_agent_runtime;

/// The rerank shadow's ledger file, under the shared shadow-ledger directory
/// — the Jev use table's name for the recall row's ledger.
pub const RERANK_SHADOW_FILE: &str = zerocode_core::jev::RECALL.ledger;

/// Outcome of a row whose judgment answered, checked out, and could be folded
/// into recall's order.
///
/// The word is the door's, because the hedge rule's sample reads it out of
/// this ledger (`JevDoor::hedge_for`) and a reader that spelled it
/// differently from the writer would find no answers at all.
pub const RERANK_OUTCOME_ANSWERED: &str = zerocode_core::jev::door::ANSWERED_OUTCOME;

/// Outcome of a row whose judgment checked out but could not be ordered: the
/// vault named two pages each other's successor, and the rule is to invent no
/// order for that. Kept apart from a failure because the judgment did its part.
pub const RERANK_OUTCOME_UNORDERABLE: &str = "unorderable";

/// The judgment's deadline on the record-only road. The same as the probe's: a
/// recall's notes are a smaller state than a task's text, and nothing waits on
/// this either way.
pub const RERANK_SHADOW_DEADLINE: Duration = PROBE_TIMEOUT;

/// The wall on the apply road, where a turn IS waiting. The routing judgment's
/// own active wall, because it is the same question asked twice — how long a Jev
/// answer may hold the thing it is deciding — and one answer to it.
pub const RERANK_APPLY_DEADLINE: Duration = super::decision_shadow::DECISION_ACTIVE_DEADLINE;

const FAIL_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

/// Where a project's rerank shadow ledger lives.
#[must_use]
pub fn rerank_shadow_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, RERANK_SHADOW_FILE)
}

/// One recall's row: what recall chose, what the judgment would have done, and
/// how the call went.
///
/// No query text, no summary, no path. The query is a fingerprint, and the
/// notes are named by slug — a page's name in the vault, not a line anyone
/// typed — because an order is unreadable without names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RerankShadowRow {
    /// Unix milliseconds when the row was made.
    pub at: u64,
    /// Fingerprint of the request text the notes were judged against.
    pub query: u64,
    /// Fingerprint of the notes in recall's order, names and summaries both,
    /// so a row is only ever compared with rows about the same reading.
    pub notes: u64,
    pub rubric_version: u32,
    /// [`RERANK_OUTCOME_ANSWERED`], [`RERANK_OUTCOME_UNORDERABLE`], or the
    /// failure's ledger token.
    pub outcome: String,
    /// How many notes were put to the judgment.
    pub candidates: usize,
    /// True when the judgment was recalled from this process's memo rather
    /// than asked; the timing fields then say nothing.
    #[serde(default)]
    pub cached: bool,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub retries: u32,
    /// The model that answered, as the response named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Requests this reading sent: none when the door refused it or the memo
    /// answered, one plus its retries when it left. Absent on a row from before
    /// the door. Spelled as every Jev ledger spells it.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "requests")]
    pub requests: Option<u32>,
    /// Lines the door withheld from what was sent — the key every row since
    /// the door carries, spelled as every Jev ledger spells it.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "redactedLines")]
    pub redacted_lines: Option<u32>,
    /// Which of the reply's rules refused it, on a row whose `outcome` is
    /// `schema` — [`runtime::memory::rerank::RerankRejection::rule`]'s word. One word for nine rules
    /// says a reply was refused but not by what, and the ledger is where the
    /// cause has to be readable. Never a word of the reply, the notes or the
    /// request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
    /// Which note the rule broke on, by its place in recall's order. Absent
    /// when the rule names no note of ours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_at: Option<usize>,
    /// When a second request of this reading was planned to leave, on a road
    /// that planned one. Absent where the rule named no delay — too few
    /// samples, an ordinary answer already past the wall, or a day whose
    /// budget could not carry a second request.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "hedgeDelayMs")]
    pub hedge_delay_ms: Option<u64>,
    /// Whether that second request actually left: no answer had come by the
    /// delay.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "hedgeFired")]
    pub hedge_fired: Option<bool>,
    /// Whether the second copy is the one that answered.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "hedgeWon")]
    pub hedge_won: Option<bool>,
    /// The losing copy's own latency, when it had answered by the time the
    /// winner was read — the column that lets two copies being slow together
    /// be measured rather than assumed away.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "loserMs")]
    pub loser_ms: Option<u64>,
    /// What the judgment said, once it checked out and could be ordered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judged: Option<Judged>,
    /// Whether [`Judged::proposed`] is the order the turn actually read. False
    /// on every record-only row, and false on an apply row whose judgment
    /// missed the wall, was refused, failed its checks, or could not be proved a
    /// permutation of what recall admitted. Absent on a row written before the
    /// apply road existed, which read as what it was: not applied.
    #[serde(default)]
    pub applied: bool,
}

/// A checked judgment folded into recall's order, and the readings it came
/// from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judged {
    /// Recall's own order — what the turn read unless `applied` says the
    /// judgment's is what it read instead.
    pub recalled: Vec<String>,
    /// The judgment's order after the graph's rules, without the notes
    /// `dropped` names. Shorter than `recalled` whenever anything dropped.
    pub proposed: Vec<String>,
    pub moved: usize,
    pub top_changed: bool,
    /// Notes the graph pinned against the judgment. The number a later phase
    /// reads before letting a judgment reorder anything for real.
    pub held_by_graph: Vec<String>,
    /// Notes the judgment put on the bottom level and the graph said nothing
    /// about: left out of `proposed`, and out of what an applying turn reads.
    /// Absent on a row from before the rule, which reads as what it was:
    /// nothing dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
    /// Notes that rule would have dropped, that the graph's claim kept. The
    /// second number `dropped` has to be read with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_by_graph: Vec<String>,
    /// Each note's `(normalised score, confidence)`, in recall's order.
    pub readings: Vec<(f64, f64)>,
}

impl Judged {
    fn from_comparison(comparison: RerankComparison, readings: &[RerankReading]) -> Self {
        Self {
            recalled: comparison.recalled,
            proposed: comparison.proposed,
            moved: comparison.moved,
            top_changed: comparison.top_changed,
            held_by_graph: comparison.held_by_graph,
            dropped: comparison.dropped,
            kept_by_graph: comparison.kept_by_graph,
            readings: readings
                .iter()
                .map(|reading| (reading.normalised, reading.confidence))
                .collect(),
        }
    }
}

impl RerankShadowRow {
    /// What this row's call cost, as the client counted it — the same account
    /// the routing row keeps (`decision_shadow::DecisionShadowRow::spent`),
    /// so a reader of one ledger can read the other.
    ///
    /// `hedge` is the delay that was planned, whether or not a second request
    /// reached it; `call.hedge` is the one that left.
    fn spent(&mut self, call: &SystemOneCall, hedge: Option<Duration>, withheld: u32) {
        self.elapsed_ms = jev_gate::millis(call.elapsed);
        self.retries = call.retries;
        // What left the machine, as the client counted it. One plus the
        // retries stopped being that number the moment a judgment could be
        // asked twice.
        self.requests = Some(call.requests);
        self.redacted_lines = Some(withheld);
        self.hedge_delay_ms = hedge.map(jev_gate::millis);
        self.hedge_fired = hedge.map(|_| call.hedge.is_some());
        self.hedge_won = call.hedge.map(|ran| ran.won);
        self.loser_ms = call.hedge.and_then(|ran| ran.loser_ms);
    }

    fn new(key: MemoKey, candidates: usize, outcome: String) -> Self {
        Self {
            at: unix_millis(),
            query: key.query,
            notes: key.notes,
            rubric_version: key.rubric,
            outcome,
            candidates,
            cached: false,
            elapsed_ms: 0,
            retries: 0,
            model: None,
            input_tokens: None,
            requests: Some(0),
            hedge_delay_ms: None,
            hedge_fired: None,
            hedge_won: None,
            loser_ms: None,
            redacted_lines: Some(0),
            rejected: None,
            rejected_at: None,
            judged: None,
            applied: false,
        }
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// A reading's judgment as this process remembers it. The key carries the
/// rubric version and the requested model, so a judgment made under other
/// words or by another model is never recalled for this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MemoKey {
    query: u64,
    notes: u64,
    rubric: u32,
    model: &'static str,
}

impl MemoKey {
    fn for_reading(query: &str, hits: &[MemoryHit]) -> Self {
        let mut notes = String::new();
        for hit in hits {
            notes.push_str(&hit.entry.slug);
            notes.push('\u{1f}');
            notes.push_str(&hit.entry.summary);
            notes.push('\u{1e}');
        }
        Self {
            query: task_fingerprint(query, ""),
            notes: task_fingerprint("", &notes),
            rubric: RERANK_RUBRIC_VERSION,
            model: SYSTEMONE_MODEL,
        }
    }
}

#[derive(Debug, Clone)]
struct Remembered {
    model: String,
    outcome: &'static str,
    judged: Option<Judged>,
}

fn memo() -> &'static Mutex<HashMap<MemoKey, Remembered>> {
    static MEMO: OnceLock<Mutex<HashMap<MemoKey, Remembered>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The seat beside recall. Seated once per host at the project's `cwd`, where
/// the setting and the ledger are; it reads that setting per recall and takes
/// the road it names — asking nothing, asking off the turn's thread, or asking
/// and waiting for the order the turn reads (`settle`).
#[derive(Debug, Clone)]
pub struct RerankShadow {
    cwd: PathBuf,
}

impl RerankShadow {
    #[must_use]
    pub fn at(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
        }
    }
}

impl RecallSeat for RerankShadow {
    fn settle(&self, query: &str, hits: Vec<MemoryHit>) -> Vec<MemoryHit> {
        settle(&self.cwd, query, hits)
    }
}

/// Everything one recall's judgment carries off the calling thread.
struct Shot {
    cwd: PathBuf,
    ledger: PathBuf,
    config: Result<SystemOneConfig, SystemOneFailure>,
    query: String,
    hits: Vec<MemoryHit>,
    deadline: Duration,
}

/// Where a waiting caller's row comes back. A rendezvous channel with no
/// buffer, so a send that succeeds is one a caller is holding: that caller
/// writes the row, and the `applied` it writes is the truth about what the turn
/// read.
type RowSender = SyncSender<RerankShadowRow>;

/// One recall, on the road its mode names: nothing, a row beside what the turn
/// read, or the order the turn reads.
///
/// The setting is read here, on the thread recall already runs on, because the
/// answer decides whether anything is cloned or spawned at all — an `off`
/// recall now copies no hits and starts no task.
pub(super) fn settle(cwd: &Path, query: &str, hits: Vec<MemoryHit>) -> Vec<MemoryHit> {
    let Some(mode) = asking_mode(cwd, query, &hits) else {
        return hits;
    };
    // `auto` acts on the standing its own ledger recorded (§4, t-5806): the
    // judge wrote a rise there when the window cleared every line on the
    // seat's own labels, and reading it back here is what makes `auto` a
    // word that decides rather than a second spelling of `shadow`. Read only
    // under `auto`: a person's `on` needs no ledger, and `shadow` reads none.
    let raised = mode == zerocode_core::jev::JevMode::Auto && runtime::jev_seat_applies(cwd, &RECALL);
    if !mode.applies_with(raised) {
        fire(cwd, query, &hits, RERANK_SHADOW_DEADLINE, None);
        return hits;
    }
    apply(cwd, query, hits)
}

/// Judge the seat on what it has just written — a reading or a label — and
/// write down a rise or a fall in this same ledger (§4), through the one
/// judge a seat with its own marks takes.
pub(super) fn judge_ledger(ledger: &Path, now_ms: i64) -> Option<zerocode_core::jev::promote::Verdict> {
    judge_seat_ledger(&RECALL, ledger, now_ms)
}

/// The clock the judge's transition rows carry.
fn now_ms() -> i64 {
    i64::try_from(unix_millis()).unwrap_or(i64::MAX)
}

/// The judge, off the turn: it reads the whole ledger — 4.5 ms on this
/// machine's 1,005-row, 1.4 MB recall ledger (measured 2026-09-22,
/// `measure_what_a_label_and_a_judge_cost_on_this_machines_ledgers`) — and
/// the road that asks for it is either holding the turn's recall thread or
/// ending the turn, neither of which should pay a file parse for a verdict
/// the NEXT recall reads. The record-only road judges inline, because it is
/// already off the turn.
fn judge_detached(ledger: PathBuf) {
    shared_agent_runtime().spawn(async move {
        let _ = tokio::task::spawn_blocking(move || judge_ledger(&ledger, now_ms())).await;
    });
}

/// The mode this recall is to be judged under, or `None` when it is not to be
/// judged at all: nothing to judge, an ablation holding the judgment out, an
/// unreadable setting, or a mode that asks nothing.
fn asking_mode(cwd: &Path, query: &str, hits: &[MemoryHit]) -> Option<zerocode_core::jev::JevMode> {
    if hits.is_empty() || query.trim().is_empty() {
        return None;
    }
    if telemetry::attest_ablated(telemetry::HarnessFeature::RerankShadow) {
        return None;
    }
    let Some(mode) = rerank_shadow_mode_from(&runtime::ConfigLoader::default_for(cwd)) else {
        telemetry::attest_failed(telemetry::HarnessFeature::RerankShadow, FAIL_SETTINGS_UNAVAILABLE);
        return None;
    };
    if !mode.asks() {
        telemetry::attest_declined(telemetry::HarnessFeature::RerankShadow, mode.key());
        return None;
    }
    Some(mode)
}

/// Put one recall to the judgment and hand back its task, telling it where to
/// send the row if anyone is waiting for it.
fn fire(
    cwd: &Path,
    query: &str,
    hits: &[MemoryHit],
    deadline: Duration,
    answer: Option<RowSender>,
) -> tokio::task::JoinHandle<()> {
    let shot = Shot {
        cwd: cwd.to_path_buf(),
        ledger: rerank_shadow_path(cwd),
        config: SystemOneConfig::from_env(),
        query: query.to_string(),
        hits: hits.to_vec(),
        deadline,
    };
    shared_agent_runtime().spawn(run(shot, answer))
}

/// A recall whose mode acts: the judgment is asked on the shared runtime and
/// the turn waits for it up to [`RERANK_APPLY_DEADLINE`].
///
/// Recall's order is what comes back from every ending but one — a judgment
/// that arrived in time, checked out, and could be proved a permutation of what
/// recall admitted. The row is written by whoever is holding it when the wall
/// passes, so a late judgment is still recorded, as one that did not apply.
fn apply(cwd: &Path, query: &str, hits: Vec<MemoryHit>) -> Vec<MemoryHit> {
    let (answer, judged) = sync_channel(0);
    fire(cwd, query, &hits, RERANK_APPLY_DEADLINE, Some(answer));
    let Ok(mut row) = judged.recv_timeout(RERANK_APPLY_DEADLINE) else {
        return hits;
    };
    let read = row
        .judged
        .as_ref()
        .and_then(|judged| apply_order(&hits, &judged.proposed, &judged.dropped));
    row.applied = read.is_some();
    let ledger = rerank_shadow_path(cwd);
    let _ = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES);
    judge_detached(ledger);
    note_settled(cwd, &row, &hits);
    read.unwrap_or(hits)
}

/// Open the door, judge the reading, and leave the row with whoever writes it.
async fn run(shot: Shot, answer: Option<RowSender>) {
    let Shot { cwd, ledger, config, query, hits, deadline } = shot;
    let opened_at = cwd.clone();
    let Ok(door) = tokio::task::spawn_blocking(move || JevDoor::open(&opened_at)).await else {
        telemetry::attest_failed(telemetry::HarnessFeature::RerankShadow, FAIL_SETTINGS_UNAVAILABLE);
        return;
    };
    let client = config.ok().map(SystemOneConfig::into_client);
    // A caller holding the other end of the rendezvous is a turn waiting for
    // this order, and a wall it is waited inside is the one thing a second
    // request can buy.
    let row = judge(&door, client.as_ref(), &query, &hits, deadline, answer.is_some()).await;
    let Some(row) = kept(row, answer).await else {
        return;
    };
    note_settled(&cwd, &row, &hits);
    let _ = tokio::task::spawn_blocking(move || {
        let written = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES);
        judge_ledger(&ledger, now_ms());
        written
    })
    .await;
}

/* ---- the label: what the turn then read ----------------------------------- */

/// The last reading this process settled for a project — what a label at the
/// turn's end is judged against.
///
/// In memory and not on disk, for the reason the skill seat gives
/// (`skill_search::last_answer`): the mark is whether THIS turn went on to
/// read what THIS reading put first, and an order read back off a ledger row
/// could be a reading another session settled an hour ago. One per project,
/// because recall runs once per request and a turn's last settled reading is
/// the one whose order the turn was handed.
struct Settled {
    query: u64,
    notes: u64,
    applied: bool,
    /// The judgment's order, each note with the path recall handed it under.
    proposed: Vec<(String, String)>,
}

fn last_settled() -> &'static Mutex<HashMap<PathBuf, Settled>> {
    static SETTLED: OnceLock<Mutex<HashMap<PathBuf, Settled>>> = OnceLock::new();
    SETTLED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember the order a reading settled on, when it settled on one: a row
/// whose judgment failed or could not be ordered proposes nothing, and a turn
/// that read recall's own order was not asked to agree with anything.
fn note_settled(cwd: &Path, row: &RerankShadowRow, hits: &[MemoryHit]) {
    let Some(judged) = row.judged.as_ref() else {
        return;
    };
    let proposed: Vec<(String, String)> = judged
        .proposed
        .iter()
        .filter_map(|slug| {
            hits.iter()
                .find(|hit| hit.entry.slug == *slug)
                .map(|hit| (slug.clone(), hit.entry.path.clone()))
        })
        .collect();
    if proposed.is_empty() {
        return;
    }
    if let Ok(mut settled) = last_settled().lock() {
        settled.insert(
            cwd.to_path_buf(),
            Settled { query: row.query, notes: row.notes, applied: row.applied, proposed },
        );
    }
}

/// The recall seat's `agreed` mark, one row per turn that was handed a
/// judged order: whether the note the judgment put FIRST was read or cited
/// before the turn ended.
///
/// A row of its own, keyed like the reading it grades (`query`, `notes`) and
/// carrying `applied` from it, so an order the turn read and an order only
/// recorded beside recall's are compared on one mark. `rank` is the place in
/// the judgment's order of the first note the turn touched, in the order the
/// turn touched them; absent when it touched none. Shaped like the skill
/// seat's label (`skill_search::SkillLabelRow`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RerankLabelRow {
    pub at: u64,
    /// The reading this row grades, spelled `<query>:<notes>` — the two
    /// fingerprints the answered row is named by.
    pub label: String,
    pub query: u64,
    pub notes: u64,
    pub applied: bool,
    pub agreed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<usize>,
}

/// Write the recall seat's mark for the turn that just ended, judged on
/// `turn` — the messages the turn appended, already in memory — against the
/// last reading this process settled for `cwd`. A note was touched when a
/// tool call named its path, or the assistant's own words cited its slug
/// (`[[slug]]`, or the path itself, as `decision_core::dreamer::cited_targets`
/// reads them). Nothing is written when no reading was settled since the
/// last label; answers whether a row was written.
#[must_use]
pub fn note_recall_read(cwd: &Path, turn: &[ConversationMessage]) -> bool {
    let Some(settled) = last_settled().lock().ok().and_then(|mut held| held.remove(cwd)) else {
        return false;
    };
    let row = label_row(&settled, turn);
    let ledger = rerank_shadow_path(cwd);
    let written = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES).is_ok();
    // A label may be the mark that clears the seat's agreement line: judge
    // now rather than at the next reading's write, so a rise the labels
    // earned is read by the very next recall — off the turn that is ending.
    judge_detached(ledger);
    written
}

/// The mark itself: whether the first proposed note was touched, and the
/// rank of the first note touched.
fn label_row(settled: &Settled, turn: &[ConversationMessage]) -> RerankLabelRow {
    let touched: Vec<usize> = touched_in_order(&settled.proposed, turn);
    RerankLabelRow {
        at: unix_millis(),
        label: format!("{}:{}", settled.query, settled.notes),
        query: settled.query,
        notes: settled.notes,
        applied: settled.applied,
        agreed: touched.contains(&0),
        rank: touched.first().copied(),
    }
}

/// The ranks of the proposed notes a turn touched, in the order it touched
/// them, each once.
fn touched_in_order(proposed: &[(String, String)], turn: &[ConversationMessage]) -> Vec<usize> {
    let mut touched = Vec::new();
    for message in turn {
        if message.role != MessageRole::Assistant {
            continue;
        }
        for block in &message.blocks {
            let mut hit = |rank: usize| {
                if !touched.contains(&rank) {
                    touched.push(rank);
                }
            };
            match block {
                ContentBlock::ToolUse { input, .. } => {
                    for (rank, (_, path)) in proposed.iter().enumerate() {
                        if !path.is_empty() && input.contains(path.as_str()) {
                            hit(rank);
                        }
                    }
                }
                ContentBlock::Text { text } => {
                    for target in decision_core::dreamer::cited_targets(text) {
                        for (rank, (slug, path)) in proposed.iter().enumerate() {
                            if cites(&target, slug, path) {
                                hit(rank);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    touched
}

/// Whether a cited target names a note: its slug, or its path whole or by a
/// tail that begins at a path component.
fn cites(target: &str, slug: &str, path: &str) -> bool {
    target == slug || target == path || path.ends_with(&format!("/{target}"))
}

/// The row when this task is the one to write it, `None` when a caller waiting
/// for the order took it instead.
async fn kept(row: RerankShadowRow, answer: Option<RowSender>) -> Option<RerankShadowRow> {
    let Some(answer) = answer else {
        return Some(row);
    };
    // A blocking send, because the handoff is a rendezvous: it finishes when a
    // caller takes the row, and fails — handing the row back — when the wall has
    // already passed and that caller has gone.
    tokio::task::spawn_blocking(move || answer.send(row).err().map(|returned| returned.0))
        .await
        .ok()
        .flatten()
}

/// One reading's row: recalled from the memo, refused at the door, or asked
/// and checked.
pub(super) async fn judge(
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    query: &str,
    hits: &[MemoryHit],
    deadline: Duration,
    waited: bool,
) -> RerankShadowRow {
    let candidates = rerank_candidates(hits);
    // The judgment is asked about at most the notes a question was built for,
    // and the fold reads only those, so the two never disagree about a note.
    let hits = &hits[..candidates.len()];
    let key = MemoKey::for_reading(query, hits);
    let recalled = memo().lock().ok().and_then(|memo| memo.get(&key).cloned());
    if let Some(remembered) = recalled {
        telemetry::attest_fired(telemetry::HarnessFeature::RerankShadow);
        let mut row = RerankShadowRow::new(key, hits.len(), remembered.outcome.to_string());
        row.cached = true;
        row.model = Some(remembered.model);
        row.judged = remembered.judged;
        return row;
    }
    let state = rerank_state(query, &candidates);
    let questions = rerank_questions(&candidates);
    let request = SystemOneRequest {
        state: &state,
        model: SYSTEMONE_MODEL,
        questions: &questions,
    };
    let Some(body) = jev_gate::body_of(&request) else {
        let failure = SystemOneFailure::InvalidRequest;
        telemetry::attest_failed(telemetry::HarnessFeature::RerankShadow, failure.token());
        return RerankShadowRow::new(key, hits.len(), failure.ledger_token());
    };
    let (cleared, client) = match (door.pass(&RECALL, client.is_some(), body), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        // The door refuses a keyless request before anything else it asks.
        (passed, _) => {
            let refused = passed.err().unwrap_or(Refused::NoKey);
            telemetry::attest_declined(telemetry::HarnessFeature::RerankShadow, refused.token());
            return RerankShadowRow::new(key, hits.len(), refused.token().to_string());
        }
    };
    let withheld = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    let hedge = door.hedge_now(&RECALL, deadline, waited);
    let call = jev_gate::send(client, cleared, deadline, hedge).await;
    // Read, not taken: the call is read for its answer here and for what it
    // cost at the end, and a judgment asked twice has more to say about the
    // cost than the answer does.
    let mut row = match &call.outcome {
        Ok(response) => {
            let checked = validate_rerank(&candidates, response);
            let mut row = match checked {
                Ok(readings) => {
                    telemetry::attest_fired(telemetry::HarnessFeature::RerankShadow);
                    let (outcome, judged) = compare(hits, &readings).map_or(
                        (RERANK_OUTCOME_UNORDERABLE, None),
                        |comparison| {
                            (RERANK_OUTCOME_ANSWERED, Some(Judged::from_comparison(comparison, &readings)))
                        },
                    );
                    if let Ok(mut memo) = memo().lock() {
                        remember_bounded(
                            &mut memo,
                            vec![(
                                key,
                                Remembered { model: response.model.clone(), outcome, judged: judged.clone() },
                            )],
                        );
                    }
                    let mut row = RerankShadowRow::new(key, hits.len(), outcome.to_string());
                    row.judged = judged;
                    row
                }
                Err(refused) => {
                    telemetry::attest_failed(
                        telemetry::HarnessFeature::RerankShadow,
                        SystemOneFailure::Schema.token(),
                    );
                    let mut row =
                        RerankShadowRow::new(key, hits.len(), SystemOneFailure::Schema.ledger_token());
                    row.rejected = Some(refused.rule().to_string());
                    row.rejected_at = refused.position();
                    row
                }
            };
            // An answer that arrived billed, whether or not it checked out.
            row.model = Some(response.model.clone());
            row.input_tokens = Some(response.usage.input_tokens);
            row
        }
        Err(failure) => {
            telemetry::attest_failed(telemetry::HarnessFeature::RerankShadow, failure.token());
            RerankShadowRow::new(key, hits.len(), failure.ledger_token())
        }
    };
    row.spent(&call, hedge, withheld);
    row
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use runtime::memory::rerank::RERANK_LEVELS;
    use runtime::MemoryEntry;

    use super::*;

    fn hit(slug: &str, summary: &str) -> MemoryHit {
        MemoryHit {
            entry: MemoryEntry {
                slug: slug.to_string(),
                path: format!("/Users/someone/.zo/projects/p/memory/{slug}.md"),
                summary: summary.to_string(),
            },
            score: 0,
        }
    }

    /// A score answer whose whole probability sits on `level`, with the legend
    /// the contract echoes back.
    fn answer(level: usize) -> serde_json::Value {
        let numbers = ["0", "1", "2", "3"];
        let probabilities: serde_json::Map<String, serde_json::Value> = numbers
            .iter()
            .enumerate()
            .map(|(index, number)| ((*number).to_string(), serde_json::json!(if index == level { 1.0 } else { 0.0 })))
            .collect();
        let legend: serde_json::Map<String, serde_json::Value> = numbers
            .iter()
            .enumerate()
            .map(|(index, number)| ((*number).to_string(), serde_json::json!(RERANK_LEVELS[index])))
            .collect();
        let score = [0.0, 1.0, 2.0, 3.0][level];
        serde_json::json!({
            "type": "score",
            "score": score,
            "confidence": 0.9,
            "legend": legend,
            "probabilities": probabilities,
        })
    }

    /// A reply that answers every note the request asked about.
    fn reply_for(levels: &[usize]) -> String {
        let answers: serde_json::Map<String, serde_json::Value> = levels
            .iter()
            .enumerate()
            .map(|(position, level)| (format!("n{position}"), answer(*level)))
            .collect();
        serde_json::json!({
            "model": "jev-test",
            "answers": answers,
            "usage": {"input_tokens": 321, "output_tokens": 12}
        })
        .to_string()
    }

    /// One scripted HTTP answer on a loopback port, recording each request
    /// body it saw. `std::net` on a thread, because the tools crate's tokio
    /// has no network driver of its own — the client brings its own.
    struct Mock {
        base_url: String,
        bodies: Arc<Mutex<Vec<String>>>,
    }

    impl Mock {
        /// A port that accepts and never answers — a judgment that misses any
        /// wall put in front of it.
        fn silent() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
            let base_url = format!("http://{}", listener.local_addr().expect("mock address"));
            let bodies = Arc::new(Mutex::new(Vec::new()));
            std::thread::spawn(move || {
                let held: Vec<std::net::TcpStream> = listener.incoming().flatten().collect();
                drop(held);
            });
            Self { base_url, bodies }
        }

        /// A port whose first answer is `delay` late and whose later answers
        /// come at once — one judgment slow on the wire, its second copy not.
        ///
        /// Each connection is answered on its own thread, because a hedge
        /// holds two of them open at the same moment and a server that
        /// answered them in turn would be measuring itself.
        fn slow_first(delay: Duration, body: String) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
            let base_url = format!("http://{}", listener.local_addr().expect("mock address"));
            let bodies = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&bodies);
            std::thread::spawn(move || {
                for (nth, stream) in listener.incoming().enumerate() {
                    let Ok(mut stream) = stream else { break };
                    let recorder = Arc::clone(&recorder);
                    let body = body.clone();
                    std::thread::spawn(move || {
                        let request = read_request(&mut stream);
                        if let Ok(mut seen) = recorder.lock() {
                            seen.push(request);
                        }
                        if nth == 0 {
                            std::thread::sleep(delay);
                        }
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
                }
            });
            Self { base_url, bodies }
        }

        fn serving(status: u16, body: String) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
            let base_url = format!("http://{}", listener.local_addr().expect("mock address"));
            let bodies = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&bodies);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let request = read_request(&mut stream);
                    if let Ok(mut seen) = recorder.lock() {
                        seen.push(request);
                    }
                    let response = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            });
            Self { base_url, bodies }
        }

        fn requests(&self) -> Vec<String> {
            self.bodies.lock().map(|seen| seen.clone()).unwrap_or_default()
        }
    }

    /// The body of one HTTP/1.1 request: headers up to the blank line, then
    /// exactly `Content-Length` bytes.
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut chunk).unwrap_or(0);
            if read == 0 {
                return String::from_utf8_lossy(&buffer).into_owned();
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        while buffer.len() < header_end + length {
            let read = stream.read(&mut chunk).unwrap_or(0);
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8_lossy(&buffer[header_end..]).into_owned()
    }

    /// The workspace every reading here comes from, consented at a door that
    /// reads no machine's settings.
    const WORKSPACE: &str = "/work/zo";

    fn door(consented: &[&str], home: &Path) -> JevDoor {
        let settings = zerocode_core::jev::door::JevSettings {
            enabled: true,
            workspaces: consented.iter().map(|root| (*root).to_string()).collect(),
            daily_requests: None,
        };
        JevDoor::at(settings, Path::new(WORKSPACE), home)
    }

    fn judged(client: &SystemOneClient, query: &str, hits: &[MemoryHit]) -> RerankShadowRow {
        let home = tempfile::tempdir().expect("a config home");
        let door = door(&[WORKSPACE], home.path());
        shared_agent_runtime().block_on(judge(&door, Some(client), query, hits, Duration::from_secs(5), false))
    }

    #[test]
    fn an_answered_reading_writes_recalls_order_beside_the_judgments_and_what_the_graph_held() {
        let mock = Mock::serving(200, reply_for(&[1, 3, 1]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let hits = [
            hit("wiki/new", "the current decision"),
            hit("wiki/old", "superseded by [[wiki/new]]"),
            hit("wiki/aside", "background only"),
        ];

        let row = judged(&client, "why was this decided (row one)", &hits);

        assert_eq!(row.outcome, RERANK_OUTCOME_ANSWERED);
        assert_eq!(row.candidates, 3);
        assert_eq!(row.model.as_deref(), Some("jev-test"));
        assert_eq!(row.input_tokens, Some(321));
        assert!(!row.cached && row.retries == 0);
        let judged = row.judged.expect("a checked judgment folds into an order");
        assert_eq!(judged.recalled, ["wiki/new", "wiki/old", "wiki/aside"]);
        assert_eq!(
            judged.proposed,
            ["wiki/new", "wiki/old", "wiki/aside"],
            "the judgment liked the replaced page most, and the graph still put its successor first"
        );
        assert_eq!(judged.held_by_graph, ["wiki/new", "wiki/old"]);
        assert!(!judged.top_changed && judged.moved == 0);
        assert_eq!(judged.readings.len(), 3);

        let sent = mock.requests();
        assert_eq!(sent.len(), 1, "one request carries every note's question");
        let body: serde_json::Value = serde_json::from_str(&sent[0]).expect("a JSON request");
        assert_eq!(body["questions"]["n1"]["type"], "score");
        assert_eq!(body["state"]["notes"][1]["name"], "wiki/old");
        assert!(
            !sent[0].contains("/Users/"),
            "a store path never leaves the machine: {}",
            sent[0]
        );
    }

    #[test]
    fn the_row_carries_fingerprints_and_names_and_never_the_words() {
        let mock = Mock::serving(200, reply_for(&[2, 2]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let hits = [
            hit("wiki/a", "SUMMARY-SENTINEL-ALPHA"),
            hit("wiki/b", "SUMMARY-SENTINEL-BETA"),
        ];

        let row = judged(&client, "QUERY-SENTINEL (row two)", &hits);
        let line = serde_json::to_string(&row).expect("a ledger line");

        assert!(line.contains("wiki/a") && line.contains("wiki/b"));
        for sentinel in ["QUERY-SENTINEL", "SUMMARY-SENTINEL", "/Users/"] {
            assert!(!line.contains(sentinel), "{sentinel} must not be in the ledger: {line}");
        }
        assert_ne!(row.query, 0);
        assert_ne!(row.notes, 0);
        assert_eq!(row.rubric_version, RERANK_RUBRIC_VERSION);
    }

    #[test]
    fn a_reply_that_fails_the_checks_is_a_schema_row_that_still_bills() {
        // Every note answered at level 1 but with the score of level 3: the
        // shape is right and the arithmetic is not.
        let mut broken: serde_json::Value = serde_json::from_str(&reply_for(&[1])).expect("json");
        broken["answers"]["n0"]["score"] = serde_json::json!(3.0);
        let mock = Mock::serving(200, broken.to_string());
        let client = SystemOneClient::new(&mock.base_url, "test-key");

        let row = judged(&client, "anything (row three)", &[hit("wiki/a", "one")]);

        assert_eq!(row.outcome, SystemOneFailure::Schema.ledger_token());
        assert!(row.judged.is_none());
        assert_eq!(row.input_tokens, Some(321), "an answer that arrived and failed still billed");
    }

    /// `schema` is one word for nine rules. A row that says only that leaves a
    /// reader to guess which check refused the reply, so it names the rule and
    /// the note the rule broke on — and still not one word of the reply.
    #[test]
    fn a_schema_row_names_the_rule_it_broke_and_the_note_it_broke_on() {
        let mut broken: serde_json::Value =
            serde_json::from_str(&reply_for(&[1, 1, 1])).expect("json");
        broken["answers"]["n2"]["score"] = serde_json::json!(3.0);
        let mock = Mock::serving(200, broken.to_string());
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let hits = [
            hit("wiki/a", "SUMMARY-SENTINEL-ALPHA"),
            hit("wiki/b", "SUMMARY-SENTINEL-BETA"),
            hit("wiki/c", "SUMMARY-SENTINEL-GAMMA"),
        ];

        let row = judged(&client, "QUERY-SENTINEL (row nine)", &hits);

        assert_eq!(row.outcome, SystemOneFailure::Schema.ledger_token());
        assert_eq!(row.rejected.as_deref(), Some("score_mismatch"));
        assert_eq!(row.rejected_at, Some(2), "the third note in recall's order");
        let line = serde_json::to_string(&row).expect("a ledger line");
        for sentinel in ["QUERY-SENTINEL", "SUMMARY-SENTINEL", "/Users/"] {
            assert!(!line.contains(sentinel), "{sentinel} must not be in the ledger: {line}");
        }
    }

    /// A row that is not a refused reply carries no rule at all — the key is
    /// absent rather than empty, so a reader counting rules counts refusals.
    #[test]
    fn a_row_nothing_refused_names_no_rule() {
        let mock = Mock::serving(200, reply_for(&[1, 3]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");

        let row = judged(&client, "why (row ten)", &[hit("wiki/a", "one"), hit("wiki/b", "two")]);

        assert_eq!(row.outcome, RERANK_OUTCOME_ANSWERED);
        assert_eq!((row.rejected.as_deref(), row.rejected_at), (None, None));
        let line = serde_json::to_string(&row).expect("a ledger line");
        assert!(!line.contains("rejected"), "an answered row keeps no refusal key: {line}");
    }

    #[test]
    fn a_refused_key_is_a_failure_row_with_nothing_judged() {
        let mock = Mock::serving(401, r#"{"error":"unauthorized"}"#.to_string());
        let client = SystemOneClient::new(&mock.base_url, "wrong-key");

        let row = judged(&client, "anything (row four)", &[hit("wiki/a", "one")]);

        assert_ne!(row.outcome, RERANK_OUTCOME_ANSWERED);
        assert!(row.judged.is_none() && row.model.is_none() && row.input_tokens.is_none());
        assert_eq!(mock.requests().len(), 1, "a refused key is final after one request");
    }

    #[test]
    fn the_same_reading_is_asked_once_and_recalled_after() {
        let mock = Mock::serving(200, reply_for(&[3, 0]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let hits = [hit("wiki/x", "first"), hit("wiki/y", "second")];

        let first = judged(&client, "the same words (row five)", &hits);
        let second = judged(&client, "the same words (row five)", &hits);

        assert!(!first.cached && second.cached);
        assert_eq!(first.judged, second.judged);
        assert_eq!(mock.requests().len(), 1, "the memo answers the second reading");
    }

    #[test]
    fn a_vault_that_contradicts_itself_is_unorderable_not_failed() {
        let mock = Mock::serving(200, reply_for(&[3, 0]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let hits = [
            hit("wiki/p", "superseded by [[wiki/q]]"),
            hit("wiki/q", "superseded by [[wiki/p]]"),
        ];

        let row = judged(&client, "which is current (row six)", &hits);

        assert_eq!(row.outcome, RERANK_OUTCOME_UNORDERABLE);
        assert!(row.judged.is_none());
        assert_eq!(row.model.as_deref(), Some("jev-test"), "the judgment did its part");
    }

    /// A reading from a workspace nobody consented to sends nothing, and its
    /// row names why.
    #[test]
    fn a_reading_nobody_consented_to_sends_nothing_and_says_so() {
        let mock = Mock::serving(200, reply_for(&[1]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let home = tempfile::tempdir().expect("a config home");
        let door = door(&["/work/elsewhere"], home.path());

        let row = shared_agent_runtime().block_on(judge(
            &door,
            Some(&client),
            "anything (row seven)",
            &[hit("wiki/a", "one")],
            Duration::from_secs(5),
            false,
        ));

        assert_eq!(row.outcome, Refused::NotConsented.token());
        assert_eq!((row.requests, row.redacted_lines), (Some(0), Some(0)));
        assert!(mock.requests().is_empty(), "the door sent nothing");
    }

    /// A line that may carry a credential — in the request or in a note's
    /// summary — never reaches the wire, and the row counts what was withheld.
    #[test]
    fn a_credential_in_a_reading_stays_behind_the_door() {
        let mock = Mock::serving(200, reply_for(&[2, 1]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let hits = [
            hit("wiki/deploy", "how the deploy runs\nexport DEPLOY_TOKEN=sk-live-SENTINEL"),
            hit("wiki/plain", "nothing to hide"),
        ];

        let row = judged(&client, "why did it fail (row eight)\npassword: hunter2-SENTINEL", &hits);

        let sent = mock.requests();
        assert_eq!(sent.len(), 1);
        assert!(!sent[0].contains("SENTINEL"), "a credential left the door: {}", sent[0]);
        assert_eq!(row.redacted_lines, Some(2), "the request's line and the summary's");
        assert_eq!(row.requests, Some(1));
        assert_eq!(row.outcome, RERANK_OUTCOME_ANSWERED);
    }

    /// A door whose day may hold `budget` requests, with `home` as both the
    /// count file's home and the ledger the hedge's sample is read from.
    fn door_with(budget: Option<u64>, home: &Path) -> JevDoor {
        let settings = zerocode_core::jev::door::JevSettings {
            enabled: true,
            workspaces: vec![WORKSPACE.to_string()],
            daily_requests: budget,
        };
        JevDoor::at(settings, Path::new(WORKSPACE), home)
    }

    /// A ledger of answers that each took `ms`, written as this ledger's own
    /// rows — so the sample the hedge rule reads is read out of what this
    /// module actually writes, spelling included.
    fn sampled(home: &Path, answers: &[u64]) {
        let path = home.join(RECALL.ledger);
        for ms in answers {
            let mut row = RerankShadowRow::new(
                MemoKey::for_reading("a past reading", &[]),
                1,
                RERANK_OUTCOME_ANSWERED.to_string(),
            );
            row.elapsed_ms = *ms;
            row.requests = Some(1);
            append_shadow_row(&path, &row, SHADOW_LEDGER_MAX_BYTES).expect("a sample row");
        }
    }

    /// One reading on the road a turn waits on. `query` is each test's own,
    /// because the memo is this process's and a reading asked twice under one
    /// name is recalled rather than sent.
    fn waited_on(client: &SystemOneClient, door: &JevDoor, wall: Duration, query: &str) -> RerankShadowRow {
        shared_agent_runtime().block_on(judge(door, Some(client), query, &[hit("wiki/a", "one")], wall, true))
    }

    /// The day's count — one byte per request that left.
    fn counted(home: &Path) -> u64 {
        zerocode_core::jev::count::sent(&zerocode_core::jev::count::requests_path(home, "2026-09-17"))
    }

    /// A sample too small to name a rank is no plan at all: the judgment takes
    /// the road it took before, one request, and the row says nothing about a
    /// hedge it never had.
    #[test]
    fn too_few_past_answers_to_name_a_delay_asks_once() {
        let mock = Mock::serving(200, reply_for(&[1]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let home = tempfile::tempdir().expect("a config home");
        let thin = vec![300; zerocode_core::jev::hedge::MIN_SAMPLES - 1];
        sampled(home.path(), &thin);

        let row = waited_on(&client, &door_with(None, home.path()), Duration::from_secs(5), "too thin a sample");

        assert_eq!(row.outcome, RERANK_OUTCOME_ANSWERED);
        assert_eq!(row.requests, Some(1));
        assert_eq!(row.hedge_delay_ms, None, "no plan, no column");
        assert_eq!((row.hedge_fired, row.hedge_won, row.loser_ms), (None, None, None));
        assert_eq!(mock.requests().len(), 1);
        assert_eq!(counted(home.path()), 1, "one request, one place in the day");
    }

    /// A plan the first answer beats costs nothing: the delay is on the row,
    /// `hedgeFired` says no, and one request left.
    #[test]
    fn a_plan_the_first_answer_beats_leaves_once() {
        let mock = Mock::serving(200, reply_for(&[1]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let home = tempfile::tempdir().expect("a config home");
        sampled(home.path(), &[400; 10]);

        let row = waited_on(&client, &door_with(None, home.path()), Duration::from_secs(5), "a plan beaten to it");

        assert_eq!(row.outcome, RERANK_OUTCOME_ANSWERED);
        assert_eq!(row.hedge_delay_ms, Some(400), "the rank the sample holds");
        assert_eq!(row.hedge_fired, Some(false), "the answer came first");
        assert_eq!(row.requests, Some(1));
        assert_eq!(mock.requests().len(), 1);
        assert_eq!(counted(home.path()), 2, "the plan's place was taken before it could leave");
    }

    /// A judgment nobody answers: the second copy leaves at the delay, both
    /// copies are on the wire, and the day is billed for both.
    #[test]
    fn a_hedge_that_fires_sends_twice_and_the_day_counts_twice() {
        let mock = Mock::silent();
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let home = tempfile::tempdir().expect("a config home");
        sampled(home.path(), &[100; 10]);
        let wall = Duration::from_millis(600);

        let row = waited_on(&client, &door_with(None, home.path()), wall, "a judgment nobody answers");

        assert_eq!(row.outcome, SystemOneFailure::Timeout.ledger_token());
        assert_eq!(row.hedge_delay_ms, Some(100));
        assert_eq!(row.hedge_fired, Some(true), "no answer by the delay, so a second copy left");
        assert_eq!(row.requests, Some(2), "what left, as the client counted it");
        assert_eq!(row.retries, 0, "a hedge is not a retry");
        assert_eq!(counted(home.path()), 2, "both requests took a place in the day");
        assert!(row.elapsed_ms < jev_gate::millis(wall * 2), "one wall for the pair: {} ms", row.elapsed_ms);
    }

    /// The second copy answers first, and that is the judgment the row keeps:
    /// an answer inside a wall the first copy was going to miss, which is the
    /// whole reason for the second one.
    #[test]
    fn a_second_copy_that_answers_first_is_the_judgment_the_row_keeps() {
        let mock = Mock::slow_first(Duration::from_secs(3), reply_for(&[1]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let home = tempfile::tempdir().expect("a config home");
        sampled(home.path(), &[80; 10]);
        let wall = Duration::from_secs(2);

        let row = waited_on(&client, &door_with(None, home.path()), wall, "a slow first copy");

        assert_eq!(row.outcome, RERANK_OUTCOME_ANSWERED, "an answer landed inside the wall");
        assert_eq!(row.hedge_delay_ms, Some(80));
        assert_eq!((row.hedge_fired, row.hedge_won), (Some(true), Some(true)));
        assert_eq!(row.requests, Some(2));
        assert!(row.judged.is_some(), "the order a turn would read");
        assert!(
            row.elapsed_ms < jev_gate::millis(wall),
            "the first copy was not waited for: {} ms",
            row.elapsed_ms
        );
        assert_eq!(counted(home.path()), 2);
    }

    /// A day that cannot carry a second request does not plan one — and the
    /// first request still goes, and still answers.
    #[test]
    fn a_day_with_no_room_for_a_second_request_plans_none() {
        let mock = Mock::serving(200, reply_for(&[1]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let home = tempfile::tempdir().expect("a config home");
        sampled(home.path(), &[400; 10]);

        let row = waited_on(&client, &door_with(Some(1), home.path()), Duration::from_secs(5), "a spent day");

        assert_eq!(row.outcome, RERANK_OUTCOME_ANSWERED, "the judgment itself still went");
        assert_eq!(row.requests, Some(1));
        assert_eq!(row.hedge_delay_ms, None, "a plan the budget declined is no plan");
        assert_eq!(mock.requests().len(), 1);
        assert_eq!(counted(home.path()), 1);
    }

    /// The road where nothing waits plans nothing: a record-only reading is
    /// read for no order, so a second copy of it would be money for nothing.
    #[test]
    fn the_record_only_road_never_asks_twice() {
        let mock = Mock::serving(200, reply_for(&[1]));
        let client = SystemOneClient::new(&mock.base_url, "test-key");
        let home = tempfile::tempdir().expect("a config home");
        sampled(home.path(), &[400; 10]);
        let door = door_with(None, home.path());

        let row = shared_agent_runtime().block_on(judge(
            &door,
            Some(&client),
            "why was this decided (recorded)",
            &[hit("wiki/a", "one")],
            RERANK_SHADOW_DEADLINE,
            false,
        ));

        assert_eq!(row.outcome, RERANK_OUTCOME_ANSWERED);
        assert_eq!(row.hedge_delay_ms, None);
        assert_eq!(row.requests, Some(1));
        assert_eq!(counted(home.path()), 1);
    }

    /// A row from before the hedge reads back as one, and a row that carries
    /// a hedge's four columns round-trips through the spelling every Jev
    /// ledger uses.
    #[test]
    fn an_older_row_reads_back_without_the_hedges_columns() {
        let before = serde_json::json!({
            "at": 1, "query": 2, "notes": 3, "rubric_version": 1,
            "outcome": RERANK_OUTCOME_ANSWERED, "candidates": 1,
            "cached": false, "elapsed_ms": 641, "retries": 0,
        });
        let row: RerankShadowRow = serde_json::from_value(before).expect("a row from before the hedge");
        assert_eq!(row.elapsed_ms, 641);
        assert_eq!((row.hedge_delay_ms, row.hedge_fired, row.hedge_won, row.loser_ms), (None, None, None, None));

        let mut hedged = row;
        hedged.hedge_delay_ms = Some(864);
        hedged.hedge_fired = Some(true);
        hedged.hedge_won = Some(true);
        hedged.loser_ms = Some(4_259);
        let written = serde_json::to_value(&hedged).expect("a hedged row");
        assert_eq!(written["hedgeDelayMs"], 864);
        assert_eq!(written["hedgeFired"], true);
        assert_eq!(written["hedgeWon"], true);
        assert_eq!(written["loserMs"], 4_259);
        assert_eq!(
            serde_json::from_value::<RerankShadowRow>(written).expect("read back"),
            hedged
        );
    }

    #[test]
    fn nothing_to_judge_asks_nothing() {
        let dir = tempfile::tempdir().expect("a temp cwd");
        assert_eq!(asking_mode(dir.path(), "a query", &[]), None);
        assert_eq!(asking_mode(dir.path(), "   ", &[hit("wiki/a", "one")]), None);
    }

    /// The whole of what the seat reads: a config home holding one consented
    /// workspace and the recall switch set to `mode`, a key, and a mock origin.
    fn machine<T>(mode: &str, base_url: &str, body: impl FnOnce(&Path) -> T) -> T {
        let home = tempfile::tempdir().expect("a config home");
        let work = tempfile::tempdir().expect("a workspace");
        // As the filesystem spells it, which is how the door spells a cwd.
        let cwd = std::fs::canonicalize(work.path()).expect("the workspace resolved");
        std::fs::write(
            home.path().join("settings.json"),
            serde_json::json!({
                zerocode_core::jev::SMART_SETTINGS_KEY: {
                    RECALL.setting: mode,
                    "jev": {"enabled": true, "workspaces": [cwd.to_string_lossy()]},
                }
            })
            .to_string(),
        )
        .expect("a settings file");
        let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", &home.path().to_string_lossy())
            .set_also("ZO_HOME", home.path())
            .set_also("HOME", home.path())
            .set_also(core_types::paths::ZO_STATE_DIR_ENV, home.path())
            .set_also(api::SYSTEMONE_API_KEY_ENV, "test-key")
            .set_also(api::SYSTEMONE_BASE_URL_ENV, base_url);
        body(&cwd)
    }

    fn slugs(hits: &[MemoryHit]) -> Vec<String> {
        hits.iter().map(|hit| hit.entry.slug.clone()).collect()
    }

    /// Rows this project's ledger holds, oldest first.
    fn rows(cwd: &Path) -> Vec<RerankShadowRow> {
        super::super::shadow_ledger::read_shadow_rows(&rerank_shadow_path(cwd))
    }

    /// Three notes recall put in a deliberately unhelpful order.
    fn three() -> [MemoryHit; 3] {
        [
            hit("wiki/a", "barely on topic"),
            hit("wiki/b", "the answer"),
            hit("wiki/c", "background"),
        ]
    }

    /// `on` is the one mode that changes what a turn reads, and the row it
    /// leaves says the order was applied.
    #[test]
    fn on_reads_the_judgments_order_and_its_row_says_so() {
        let mock = Mock::serving(200, reply_for(&[1, 3, 2]));
        let hits = three();

        let (read, rows) = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (row nine)", hits.to_vec());
            (read, rows(cwd))
        });

        assert_eq!(slugs(&read), ["wiki/b", "wiki/c", "wiki/a"]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].applied, "the row says the turn read the judgment's order");
        assert_eq!(rows[0].outcome, RERANK_OUTCOME_ANSWERED);
        assert_eq!(
            rows[0].judged.as_ref().expect("a judgment").proposed,
            slugs(&read),
            "the order the row names is the order the turn read"
        );
    }

    /// A record-only mode asks the same question and writes the same comparison,
    /// and the turn reads recall's order all the same.
    #[test]
    fn a_record_only_mode_leaves_recalls_order_and_says_it_did_not_apply() {
        for mode in [zerocode_core::jev::JevMode::Shadow, zerocode_core::jev::JevMode::Auto] {
            let mock = Mock::serving(200, reply_for(&[0, 3, 1]));
            let hits = three();

            let (read, rows) = machine(mode.key(), &mock.base_url, |cwd| {
                let read = settle(cwd, &format!("which note answers this ({})", mode.key()), hits.to_vec());
                // The road is detached, so the row lands after the turn moved on.
                let ledger = rerank_shadow_path(cwd);
                let waited = std::time::Instant::now();
                while !ledger.exists() && waited.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                (read, rows(cwd))
            });

            assert_eq!(slugs(&read), slugs(&hits), "{} reads recall's order", mode.key());
            assert_eq!(rows.len(), 1, "{}", mode.key());
            assert!(!rows[0].applied, "{} records and acts on nothing", mode.key());
            let judged = rows[0].judged.as_ref().expect("a judgment");
            assert!(
                judged.top_changed,
                "{} still asked, and the judgment still disagreed",
                mode.key()
            );
            assert_eq!(
                judged.dropped,
                ["wiki/a"],
                "{} writes down the note it would have left out",
                mode.key()
            );
            assert!(
                read.iter().any(|hit| hit.entry.slug == "wiki/a"),
                "{} wrote that down and the turn read the note anyway",
                mode.key()
            );
        }
    }

    #[test]
    fn off_asks_nothing_and_reads_recalls_order() {
        let mock = Mock::serving(200, reply_for(&[0, 3, 1]));
        let hits = three();

        let (read, rows) = machine(zerocode_core::jev::JevMode::Off.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (row off)", hits.to_vec());
            (read, rows(cwd))
        });

        assert_eq!(slugs(&read), slugs(&hits));
        assert!(rows.is_empty() && mock.requests().is_empty());
    }

    /// A judgment that does not arrive inside the wall leaves recall's order,
    /// and is recorded by the task it belongs to rather than lost.
    #[test]
    fn a_judgment_past_the_wall_leaves_recalls_order() {
        let mock = Mock::silent();
        let hits = three();

        let read = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            settle(cwd, "which note answers this (row ten)", hits.to_vec())
        });

        assert_eq!(slugs(&read), slugs(&hits), "Jev never makes an order worse");
    }

    /// A reply that fails the contract's checks is discarded whole, on the
    /// apply road as in shadow: recall's order stands and the row says so.
    #[test]
    fn a_reply_that_breaks_the_contract_leaves_recalls_order() {
        let mut broken: serde_json::Value =
            serde_json::from_str(&reply_for(&[0, 3, 1])).expect("json");
        broken["answers"]["n1"]["score"] = serde_json::json!(0.0);
        let mock = Mock::serving(200, broken.to_string());
        let hits = three();

        let (read, rows) = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (row eleven)", hits.to_vec());
            (read, rows(cwd))
        });

        assert_eq!(slugs(&read), slugs(&hits));
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].applied && rows[0].judged.is_none());
        assert_eq!(rows[0].outcome, SystemOneFailure::Schema.ledger_token());
    }

    /// An order that is not a permutation of what recall admitted folds
    /// nothing, and the row that named it says it did not apply.
    #[test]
    fn an_order_the_fold_cannot_prove_leaves_recalls_order() {
        let hits = three();
        let mut row = RerankShadowRow::new(
            MemoKey::for_reading("anything", &hits),
            hits.len(),
            RERANK_OUTCOME_ANSWERED.to_string(),
        );
        row.judged = Some(Judged {
            recalled: slugs(&hits),
            proposed: vec!["wiki/b".to_string(), "wiki/z".to_string(), "wiki/a".to_string()],
            moved: 3,
            top_changed: true,
            held_by_graph: Vec::new(),
            dropped: Vec::new(),
            kept_by_graph: Vec::new(),
            readings: vec![(0.0, 0.9); 3],
        });

        let read = row
            .judged
            .as_ref()
            .and_then(|judged| apply_order(&hits, &judged.proposed, &judged.dropped));

        assert_eq!(read, None, "a note recall never admitted folds nothing");
    }

    /// The apply road's whole point: a note the judgment read as bearing on
    /// nothing is one the turn does not get, and the row names it.
    #[test]
    fn the_apply_road_leaves_the_bottom_level_out_of_the_turn() {
        let mock = Mock::serving(200, reply_for(&[0, 3, 1]));
        let hits = three();

        let (read, rows) = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (row twelve)", hits.to_vec());
            (read, rows(cwd))
        });

        assert_eq!(slugs(&read), ["wiki/b", "wiki/c"]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].applied);
        let judged = rows[0].judged.as_ref().expect("a judgment");
        assert_eq!(judged.dropped, ["wiki/a"]);
        assert_eq!(
            judged.recalled.len(),
            3,
            "the row still says what recall handed over, or the drop is unreadable"
        );
    }

    /// A recall the judgment read as noise all the way down leaves the turn no
    /// recall section at all — the number this rule is really for.
    #[test]
    fn a_recall_read_as_noise_all_the_way_down_leaves_the_turn_nothing() {
        let mock = Mock::serving(200, reply_for(&[0, 0, 0]));
        let hits = three();

        let (read, rows) = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (row thirteen)", hits.to_vec());
            (read, rows(cwd))
        });

        assert!(read.is_empty());
        assert!(rows[0].applied);
        assert_eq!(
            rows[0].judged.as_ref().expect("a judgment").dropped.len(),
            3
        );
    }

    /// A row written before the drop existed carries neither list, and reads as
    /// what it was: a judgment that left everything in.
    #[test]
    fn a_row_from_before_the_rule_reads_as_nothing_dropped() {
        let hits = three();
        let before = serde_json::json!({
            "recalled": slugs(&hits),
            "proposed": ["wiki/b", "wiki/c", "wiki/a"],
            "moved": 3,
            "top_changed": true,
            "held_by_graph": [],
            "readings": [[0.0, 0.9], [1.0, 0.9], [0.3, 0.9]],
        });

        let judged: Judged = serde_json::from_value(before).expect("a row from before the rule");

        assert!(judged.dropped.is_empty() && judged.kept_by_graph.is_empty());
        assert!(
            apply_order(&hits, &judged.proposed, &judged.dropped).is_some(),
            "an order from before the rule still folds: nothing was dropped"
        );
    }

    /// What the bottom-level drop removes, replayed over the rows this switch
    /// has already written.
    ///
    /// The ledger keeps no query text and no summaries — by design — so this
    /// replays what a row CAN answer: the applied order (`judged.proposed`,
    /// after the graph has had its say), each note's reading, and the notes the
    /// graph pinned. It reports what leaves and what the graph keeps.
    ///
    /// It does NOT say whether leaving them out was right. Scoring a Jev filter
    /// with the same Jev readings the filter is made of would only show the two
    /// agreeing with themselves; what a drop is worth is a question for a
    /// measured comparison of turns, not for this.
    ///
    /// Three inexactnesses, named rather than smoothed over:
    ///
    /// * the shipped rule's exception is every note the vault's graph made a
    ///   claim about, which a row does not carry. `held_by_graph`, which it
    ///   does, is about PLACES — a claim the judgment already satisfied leaves
    ///   it empty — so the notes it names are a FLOOR under what the graph
    ///   keeps, and the drop count a ceiling.
    /// * the section is re-rendered from the vault's pages as they stand now,
    ///   not from the summaries recall rendered then, and a note whose page is
    ///   gone (or was never a vault page) is left out of the token reading and
    ///   counted as uncovered.
    /// * a row written under this rule already removed some notes, and their
    ///   places in its order are not recoverable. Its notes still count — the
    ///   note columns do not care about order — but the entry and token
    ///   columns, which are about the rendered FRONT of the order, skip it.
    ///   That is what keeps this readable as rows accumulate under the rule
    ///   instead of the sample collapsing to the rows from before it.
    ///
    /// It takes the ledger FILE rather than the project whose ledger it is,
    /// because [`rerank_shadow_path`] maps a `cwd` through `project_slug`,
    /// whose hash is a `DefaultHasher` — stable within a build and not across
    /// them, so a ledger the installed zo wrote is not at the path this test
    /// binary computes for the same directory. `rerank_shadow_path` is how to
    /// find it; the path is how to read it.
    ///
    /// ```text
    /// ZO_RERANK_REPLAY_LEDGER=~/.zo/projects/<slug>/state/smart-router/rerank-shadow.jsonl \
    ///   ZEROCODE_SECOND_BRAIN=/Users/dev/vault \
    ///   cargo test -p tools --lib -- what_the_bottom_level_drop_removes --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "replays this machine's real ledger and vault; run deliberately"]
    fn what_the_bottom_level_drop_removes() {
        let ledger = std::env::var_os("ZO_RERANK_REPLAY_LEDGER")
            .map(PathBuf::from)
            .expect("point ZO_RERANK_REPLAY_LEDGER at a rerank-shadow.jsonl");
        let vault = std::env::var_os("ZEROCODE_SECOND_BRAIN")
            .map(PathBuf::from)
            .expect("point ZEROCODE_SECOND_BRAIN at the vault");
        let written = std::fs::read_to_string(&ledger)
            .unwrap_or_else(|why| panic!("no ledger at {}: {why}", ledger.display()));
        let (rows, readings) = distinct_readings(&written);
        assert!(
            readings.len() > 20,
            "{} readings is too few to read anything off",
            readings.len()
        );

        let reach = Reach::over(&readings, &vault_pages(&vault));
        let turns = readings.len();
        println!("\n  ledger: {}", ledger.display());
        println!("  {rows} rows, {turns} distinct readings replayed");
        if reach.ordered < turns {
            println!(
                "  {} of them were written under the rule: their notes count, their order does not",
                turns - reach.ordered
            );
        }
        println!("\n  judged notes               {}", reach.notes);
        println!(
            "  bottom level, dropped      {}  = {:5.1}%",
            reach.dropped,
            share(reach.dropped, reach.notes)
        );
        println!(
            "  bottom level, graph kept   {}  (a floor; see the doc comment)",
            reach.kept_by_graph
        );
        println!(
            "  readings losing every note {}/{turns}  = {:5.1}%   <- the recall was noise whole",
            reach.lost_whole,
            share(reach.lost_whole, turns)
        );
        println!(
            "\n  rendered entries           {} -> {}  (-{}, {:5.1}%)",
            reach.entries_before,
            reach.entries_after,
            reach.entries_before - reach.entries_after,
            share(reach.entries_before - reach.entries_after, reach.entries_before)
        );
        println!(
            "  readings rendering fewer   {}/{}",
            reach.fewer_entries, reach.ordered
        );
        println!(
            "\n  section tokens             {} -> {}  (-{}, {:5.1}%)",
            reach.tokens_before,
            reach.tokens_after,
            reach.tokens_before - reach.tokens_after,
            share(reach.tokens_before - reach.tokens_after, reach.tokens_before)
        );
        println!(
            "  over {}/{} readings whose rendered pages all still resolve in the vault",
            reach.covered, reach.ordered
        );
        // The reserve does not move: it is the worst case for a full section,
        // and a turn can still render one.
        println!(
            "\n  the preflight reserve is unchanged at {} tokens — the worst case for {} capped",
            runtime::memory::recall::recall_section_reserve_tokens(),
            runtime::memory::recall::MAX_RECALLED_ENTRIES
        );
        println!(
            "  entries, and a turn can still render {}",
            runtime::memory::recall::MAX_RECALLED_ENTRIES
        );
    }

    /// A share of a sample the caller has already asserted is small.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a share of a few hundred judged notes"
    )]
    fn share(part: usize, whole: usize) -> f64 {
        100.0 * part as f64 / whole as f64
    }

    /// How many rows were read, and one judgment per distinct reading.
    ///
    /// One reading asked once. The memo answers every repeat of it, and a
    /// memoed row would count the same notes twice.
    fn distinct_readings(written: &str) -> (usize, Vec<Judged>) {
        let mut asked: std::collections::BTreeSet<(u64, u64)> = std::collections::BTreeSet::new();
        let mut replayed = Vec::new();
        let mut rows = 0usize;
        for line in written.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(row) = serde_json::from_str::<RerankShadowRow>(line) else {
                continue;
            };
            rows += 1;
            let Some(judged) = row.judged else { continue };
            if asked.insert((row.query, row.notes)) {
                replayed.push(judged);
            }
        }
        (rows, replayed)
    }

    /// The vault's pages by slug, for re-rendering a section the ledger only
    /// names.
    fn vault_pages(vault: &Path) -> HashMap<String, runtime::MemoryEntry> {
        runtime::second_brain::corpus::scan(&runtime::SecondBrain::at(vault))
            .pages
            .iter()
            .map(|page| (page.entry().slug.clone(), page.entry().clone()))
            .collect()
    }

    /// What the drop reaches across a replayed ledger.
    #[derive(Debug, Default)]
    struct Reach {
        notes: usize,
        dropped: usize,
        kept_by_graph: usize,
        /// Readings the drop empties: the recall was noise all the way down.
        lost_whole: usize,
        /// Readings whose order is known whole — the rows from before the
        /// rule. The three columns below are about the rendered FRONT of an
        /// order, so they count only these.
        ordered: usize,
        entries_before: usize,
        entries_after: usize,
        fewer_entries: usize,
        tokens_before: usize,
        tokens_after: usize,
        /// Readings of those whose rendered pages all still resolve, so the
        /// token columns are about the same readings on both sides.
        covered: usize,
    }

    impl Reach {
        fn over(readings: &[Judged], pages: &HashMap<String, runtime::MemoryEntry>) -> Self {
            let mut reach = Self::default();
            for judged in readings {
                reach.add(judged, pages);
            }
            reach
        }

        fn add(&mut self, judged: &Judged, pages: &HashMap<String, runtime::MemoryEntry>) {
            use runtime::memory::recall::MAX_RECALLED_ENTRIES;

            let reading: HashMap<&str, f64> = judged
                .recalled
                .iter()
                .map(String::as_str)
                .zip(judged.readings.iter().map(|(normalised, _)| *normalised))
                .collect();
            let held: std::collections::BTreeSet<&str> =
                judged.held_by_graph.iter().map(String::as_str).collect();
            self.notes += judged.readings.len();

            // Every note the reading admitted, whichever list the row filed it
            // under. A row written under the rule has already removed some, so
            // this is the only way to count what it was handed — and the
            // order those removed notes sat in is what it cannot say.
            let admitted: Vec<&str> = judged
                .proposed
                .iter()
                .chain(&judged.dropped)
                .map(String::as_str)
                .collect();
            let mut kept = Vec::with_capacity(admitted.len());
            for slug in &admitted {
                // The reading a row keeps is normalised; the cut is on the
                // level scale, and sits off the wire's grid so the trip back
                // cannot move a note across it.
                let bottom = reading.get(slug).is_some_and(|normalised| {
                    runtime::memory::rerank::reads_as_bottom_level(
                        normalised * runtime::memory::rerank::RERANK_TOP_LEVEL,
                    )
                });
                match (bottom, held.contains(slug)) {
                    (true, false) => self.dropped += 1,
                    (true, true) => self.kept_by_graph += 1,
                    (false, _) => {}
                }
                if !bottom || held.contains(slug) {
                    kept.push(*slug);
                }
            }
            if kept.is_empty() {
                self.lost_whole += 1;
            }
            if !judged.dropped.is_empty() {
                return;
            }
            self.ordered += 1;
            let (shown, left) = (
                admitted.len().min(MAX_RECALLED_ENTRIES),
                kept.len().min(MAX_RECALLED_ENTRIES),
            );
            self.entries_before += shown;
            self.entries_after += left;
            if left < shown {
                self.fewer_entries += 1;
            }
            if let (Some(was), Some(now)) =
                (section_tokens(&admitted, pages), section_tokens(&kept, pages))
            {
                self.tokens_before += was;
                self.tokens_after += now;
                self.covered += 1;
            }
        }
    }

    /// The section those pages render today, sized the way the compaction
    /// preflight sizes it (`recall_section_reserve_tokens`' chars/4 + 1).
    /// `None` when a page the order names is not in the vault.
    fn section_tokens(order: &[&str], pages: &HashMap<String, runtime::MemoryEntry>) -> Option<usize> {
        let hits: Option<Vec<MemoryHit>> = order
            .iter()
            .take(runtime::memory::recall::MAX_RECALLED_ENTRIES)
            .map(|slug| pages.get(*slug).cloned().map(|entry| MemoryHit { entry, score: 0 }))
            .collect();
        Some(
            runtime::render_recalled_memory_section(&hits?)
                .map_or(0, |section| section.chars().count() / 4 + 1),
        )
    }


    /// The label rows this project's ledger holds, oldest first — read as
    /// labels, so a reading's row is skipped, as a label is by `rows`.
    fn labels(cwd: &Path) -> Vec<RerankLabelRow> {
        super::super::shadow_ledger::read_shadow_rows(&rerank_shadow_path(cwd))
    }

    /// What a turn appended: one assistant message per block.
    fn turn(blocks: Vec<ContentBlock>) -> Vec<ConversationMessage> {
        std::iter::once(ConversationMessage::user_text("the question"))
            .chain(blocks.into_iter().map(|block| ConversationMessage::assistant(vec![block])))
            .collect()
    }

    fn read_of(path: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: "toolu_1".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"file_path": path}).to_string(),
        }
    }

    fn said(text: &str) -> ContentBlock {
        ContentBlock::Text { text: text.to_string() }
    }

    /// The label (t-5806): whether the note the judgment put first was read
    /// or cited before the turn ended, and the rank of the first note the
    /// turn touched. One row per settled reading, shaped like the skill
    /// seat's, carrying `applied` so an applied order and a recorded one are
    /// compared on the same mark.
    #[test]
    fn the_label_says_whether_the_turn_read_what_the_judgment_put_first() {
        let mock = Mock::serving(200, reply_for(&[1, 3, 2]));
        let hits = three();
        let (read, labeled) = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (label, row nine)", hits.to_vec());
            // The turn read the judgment's first note by its path: agreed.
            let first = read[0].entry.path.clone();
            assert!(note_recall_read(cwd, &turn(vec![read_of(&first)])));
            // Nothing settled since: a second label has nothing to grade.
            assert!(!note_recall_read(cwd, &turn(vec![read_of(&first)])));
            (read, labels(cwd))
        });
        assert_eq!(slugs(&read), ["wiki/b", "wiki/c", "wiki/a"]);
        assert_eq!(labeled.len(), 1, "{labeled:?}");
        let label = &labeled[0];
        assert_eq!((label.agreed, label.rank, label.applied), (true, Some(0), true));
        assert_eq!(label.label, format!("{}:{}", label.query, label.notes));
    }

    #[test]
    fn a_turn_that_cited_the_second_note_and_read_none_disagrees_at_rank_one() {
        let mock = Mock::serving(200, reply_for(&[1, 3, 2]));
        let hits = three();
        let labeled = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let _read = settle(cwd, "which note answers this (label, cited)", hits.to_vec());
            // `[[wiki/c]]` in the assistant's own words, and a path-shaped
            // mention of a note nobody proposed.
            assert!(note_recall_read(
                cwd,
                &turn(vec![said("see [[wiki/c|the background]] and wiki/zzz.md")])
            ));
            labels(cwd)
        });
        assert_eq!((labeled[0].agreed, labeled[0].rank), (false, Some(1)), "{labeled:?}");
    }

    #[test]
    fn a_turn_that_touched_no_proposed_note_disagrees_with_no_rank() {
        let mock = Mock::serving(200, reply_for(&[1, 3, 2]));
        let hits = three();
        let labeled = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let _read = settle(cwd, "which note answers this (label, nothing)", hits.to_vec());
            // A tool result and a user message are not the assistant's doing;
            // a read of some other file touches nothing.
            let mut messages = turn(vec![read_of("/somewhere/else.md")]);
            messages.push(ConversationMessage::user_text("[[wiki/b]] typed by the person"));
            assert!(note_recall_read(cwd, &messages));
            labels(cwd)
        });
        assert_eq!((labeled[0].agreed, labeled[0].rank), (false, None), "{labeled:?}");
    }

    /// A recorded reading (`shadow`) is labeled too, with `applied: false`,
    /// so the two populations can be compared on one mark; the row lands
    /// after the turn moved on, and the label waits for nothing it cannot
    /// have — a turn that ends before the row settled leaves no label.
    #[test]
    fn a_recorded_reading_is_labeled_once_its_row_has_settled() {
        let mock = Mock::serving(200, reply_for(&[0, 3, 1]));
        let hits = three();
        let labeled = machine(zerocode_core::jev::JevMode::Shadow.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (label, shadow)", hits.to_vec());
            assert_eq!(slugs(&read), slugs(&hits), "shadow reads recall's order");
            let waited = std::time::Instant::now();
            while rows(cwd).is_empty() && waited.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let proposed = rows(cwd)[0].judged.as_ref().expect("a judgment").proposed.clone();
            assert_eq!(proposed[0], "wiki/b");
            // The turn read the judgment's first note, though it was handed
            // recall's order — the mark says the judgment would have been
            // right to put it first.
            let first = hits.iter().find(|hit| hit.entry.slug == proposed[0]).expect("the note").entry.path.clone();
            assert!(note_recall_read(cwd, &turn(vec![read_of(&first)])));
            labels(cwd)
        });
        assert_eq!(labeled.len(), 1, "{labeled:?}");
        assert_eq!((labeled[0].agreed, labeled[0].rank, labeled[0].applied), (true, Some(0), false));
    }

    #[test]
    fn a_label_is_not_a_request_and_a_request_is_not_a_label() {
        let mock = Mock::serving(200, reply_for(&[1, 3, 2]));
        let hits = three();
        let (readings, labeled, asked) = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (label, kinds)", hits.to_vec());
            assert!(note_recall_read(cwd, &turn(vec![read_of(&read[0].entry.path)])));
            let asked = super::super::jev_summary::read_rows(&rerank_shadow_path(cwd))
                .iter()
                .filter(|row| zerocode_core::jev::summary::asked_something(row).is_some())
                .count();
            (rows(cwd), labels(cwd), asked)
        });
        assert_eq!((readings.len(), labeled.len(), asked), (1, 1, 1));
    }


    /// `auto` rises on the seat's own labels (t-5806, "모든 승격"): a window of
    /// readings that answered inside the wall and twenty turns that read what
    /// the judgment put first clear every line, the judge writes the rise in
    /// this ledger, and the very next recall under `auto` reads the
    /// judgment's order — where the recall before the rise read recall's own.
    #[test]
    fn auto_rises_on_its_own_labels_and_the_next_recall_reads_the_judgments_order() {
        use zerocode_core::jev::promote::{Verdict, ROSE};
        use zerocode_core::jev::summary::{rows_that_can_clear, JUDGED_EVERY_ROWS};
        let mock = Mock::serving(200, reply_for(&[1, 3, 2]));
        let hits = three();
        let (before, verdict, rose, after, applied) = machine(zerocode_core::jev::JevMode::Auto.key(), &mock.base_url, |cwd| {
            let ledger = rerank_shadow_path(cwd);
            // The seat starts recording: the turn reads recall's order.
            let before = settle(cwd, "which note answers this (auto, before)", hits.to_vec());
            let began = std::time::Instant::now();
            while rows(cwd).is_empty() && began.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(10));
            }
            // A window's worth of answered readings, at a judgment boundary,
            // and a window's worth of labels that agreed — written as the
            // seat writes them, in its own ledger.
            let floor = RECALL.answer_floor_permille.expect("recall rises");
            let wanted = rows_that_can_clear(floor).next_multiple_of(JUDGED_EVERY_ROWS);
            let already = rows(cwd).len();
            for at in 0..(wanted - already) {
                let row = serde_json::json!({
                    "at": 1_000 + at, "query": at, "notes": at, "rubric_version": RERANK_RUBRIC_VERSION,
                    "outcome": RERANK_OUTCOME_ANSWERED, "candidates": 3, "elapsed_ms": 300, "retries": 0,
                    "requests": 1, "redactedLines": 0, "applied": false,
                });
                append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES).expect("a reading");
            }
            for at in 0..JUDGED_EVERY_ROWS {
                let label = serde_json::json!({
                    "at": 5_000 + at, "label": format!("{at}:{at}"), "query": at, "notes": at,
                    "applied": false, "agreed": true, "rank": 0,
                });
                append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).expect("a label");
            }
            let verdict = judge_ledger(&ledger, 9_000);
            let rose = super::super::jev_summary::read_rows(&ledger)
                .iter()
                .any(|row| zerocode_core::jev::summary::TRANSITION.read(row) == Some(&serde_json::json!(ROSE)));
            // Raised: the next recall under `auto` reads the judgment's order.
            let after = settle(cwd, "which note answers this (auto, after)", hits.to_vec());
            let applied = rows(cwd).last().map(|row| row.applied);
            (before, verdict, rose, after, applied)
        });
        assert_eq!(slugs(&before), slugs(&hits), "a recording seat reads recall's order");
        assert_eq!(verdict, Some(Verdict::Rise), "the labels did not raise the seat");
        assert!(rose, "the rise was not written in the seat's own ledger");
        assert_eq!(slugs(&after), ["wiki/b", "wiki/c", "wiki/a"], "the raised seat did not act");
        assert_eq!(applied, Some(true), "the row of the raised seat's reading does not say it applied");
    }
}
