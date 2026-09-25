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

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{
    SystemOneCall, SystemOneClient, SystemOneConfig, SystemOneFailure, SystemOneRequest,
    SYSTEMONE_MODEL,
};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::RECALL;
use runtime::memory::recall::{RecallDemand, RecallDemandSource, MAX_RECALLED_ENTRIES};
use runtime::memory::rerank::{
    apply_order, compare, rerank_candidates, rerank_questions, rerank_state, validate_rerank,
    RerankComparison, RerankReading, MAX_RERANK_CANDIDATES, RERANK_RUBRIC_VERSION,
};
use core_types::{ContentBlock, ConversationMessage, MessageRole};
use runtime::{MemoryHit, RecallSeat};
use serde::{Deserialize, Serialize};

use super::jev_gate::{self, JevDoor};
use super::probe_exec::{remember_bounded, task_fingerprint, PROBE_TIMEOUT};
use super::settings::rerank_shadow_mode_from;
use super::turn_reads::read_path;
use super::shadow_ledger::{
    append_shadow_row, judge_seat_ledger, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES, TAIL_ROW_BYTES,
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
/// rubric version and the requested model — the door's, pinned or not
/// ([`jev_gate::model_key`]) — so a judgment made under other words or by
/// another model is never recalled for this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MemoKey {
    query: u64,
    notes: u64,
    rubric: u32,
    model: u64,
}

impl MemoKey {
    fn for_reading(query: &str, hits: &[MemoryHit], model: u64) -> Self {
        let (query, notes) = reading_fingerprints(query, hits);
        Self { query, notes, rubric: RERANK_RUBRIC_VERSION, model }
    }
}

/// The two fingerprints a reading's row is named by, and its label with it
/// (`RerankLabelRow::label`): the request text, and the notes in recall's
/// order — names and summaries both. `hits` is the notes the question is
/// built for, [`MAX_RERANK_CANDIDATES`] at most.
fn reading_fingerprints(query: &str, hits: &[MemoryHit]) -> (u64, u64) {
    let mut notes = String::new();
    for hit in hits {
        notes.push_str(&hit.entry.slug);
        notes.push('\u{1f}');
        notes.push_str(&hit.entry.summary);
        notes.push('\u{1e}');
    }
    (task_fingerprint(query, ""), task_fingerprint("", &notes))
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
    fn settle(&self, attempt: &str, query: &str, hits: Vec<MemoryHit>) -> Vec<MemoryHit> {
        settle(&self.cwd, attempt, query, hits)
    }

    fn observe(&self, attempt: &str, progress: runtime::TurnProgress<'_>) {
        heard(&self.cwd, attempt, progress);
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
    label: ReadingSlot,
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
pub(super) fn settle(cwd: &Path, attempt: &str, query: &str, hits: Vec<MemoryHit>) -> Vec<MemoryHit> {
    let label = reading_slot(cwd, attempt);
    let Some(mode) = asking_mode(cwd, query, &hits) else {
        return hits;
    };
    // Named before the road forks, over the notes the question is built for,
    // so the label a turn with no judgment writes is still named for the row
    // this reading leaves (`reading_fingerprints`).
    let key = reading_fingerprints(query, &hits[..hits.len().min(MAX_RERANK_CANDIDATES)]);
    let read = if seat_acts(cwd, mode) {
        apply(cwd, query, hits, &label)
    } else {
        fire(cwd, query, &hits, RERANK_SHADOW_DEADLINE, None, Arc::clone(&label));
        hits
    };
    // What the turn is about to read, whichever road handed it over — the
    // one showing per turn the label counts each note as (t-6264).
    note_shown(&label, key, &read);
    read
}

/// Whether the seat acts on this project's recalls under `mode` (§4): a
/// person's `on`, or an `auto` standing on what its own ledger recorded
/// (t-5806) — the judge wrote a rise there when the window cleared every
/// line on the seat's own labels, and reading it back here is what makes
/// `auto` a word that decides rather than a second spelling of `shadow`.
/// Read only under `auto`: a person's `on` needs no ledger, and `shadow`
/// reads none.
///
/// One reading, because two roads ask it of one seat: the recall the seat
/// settles, and the demand the retriever asks for before that recall
/// (`demand_for`). Two spellings would be a seat that ranks on its rows
/// while recording, or records while ranking.
fn seat_acts(cwd: &Path, mode: zerocode_core::jev::JevMode) -> bool {
    let raised = mode == zerocode_core::jev::JevMode::Auto && raised_now(cwd);
    mode.applies_with(raised)
}

/// `runtime::jev_seat_applies` for this seat, asked once per state of its
/// ledger: the retriever asks it for the demand and the seat asks it again
/// for the road a moment later, on the same recall, and nothing is written
/// between the two. The whole-ledger read it costs (4.5 ms on this
/// machine's 1,005-row ledger, t-5806) is paid once per recall under `auto`,
/// as it was before the demand asked too.
///
/// A state is what one look at the ledger sees ([`LedgerLook`]) — the same
/// witness the demand's fold keeps, so the two caches tell a replaced ledger
/// apart the same way. A standing is kept only when the ledger held still
/// while the common reader read it: one replaced between the two looks may
/// have been read as either, and is read again next time (t-6264).
fn raised_now(cwd: &Path) -> bool {
    static STAND: OnceLock<Mutex<StandingBook>> = OnceLock::new();
    let ledger = rerank_shadow_path(cwd);
    let before = LedgerLook::of(&ledger);
    let memo = STAND.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((seen, raised)) = memo.lock().ok().as_ref().and_then(|held| held.get(&ledger)) {
        if *seen == before {
            return *raised;
        }
    }
    let raised = runtime::jev_seat_applies(cwd, &RECALL);
    let held_still = LedgerLook::of(&ledger) == before;
    if let Ok(mut held) = memo.lock() {
        if held_still {
            held.insert(ledger, (before, raised));
        } else {
            held.remove(&ledger);
        }
    }
    raised
}

/// Each ledger's standing as last read, beside the look it was read under.
type StandingBook = HashMap<PathBuf, (Option<LedgerLook>, bool)>;

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
    label: ReadingSlot,
) -> tokio::task::JoinHandle<()> {
    let shot = Shot {
        cwd: cwd.to_path_buf(),
        ledger: rerank_shadow_path(cwd),
        config: SystemOneConfig::from_env(),
        query: query.to_string(),
        hits: hits.to_vec(),
        deadline,
        label,
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
fn apply(cwd: &Path, query: &str, hits: Vec<MemoryHit>, label: &ReadingSlot) -> Vec<MemoryHit> {
    let (answer, judged) = sync_channel(0);
    fire(cwd, query, &hits, RERANK_APPLY_DEADLINE, Some(answer), Arc::clone(label));
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
    note_settled(label, &row, &hits);
    read.unwrap_or(hits)
}

/// Open the door, judge the reading, and leave the row with whoever writes it.
async fn run(shot: Shot, answer: Option<RowSender>) {
    let Shot { cwd, ledger, config, query, hits, deadline, label } = shot;
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
    note_settled(&label, &row, &hits);
    let _ = tokio::task::spawn_blocking(move || {
        let written = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES);
        judge_ledger(&ledger, now_ms());
        written
    })
    .await;
}

/* ---- the label: what the turn then read ----------------------------------- */

/// What one attempt's recalls have put in front of its turn so far, what the
/// turn has done since, and the last reading settled on any of them — what
/// its turn-end label grades.
///
/// In memory and not on disk, for the reason the skill seat gives
/// (`skill_search::last_answer`): the mark is whether THIS turn went on to
/// read what THIS reading put first, and an order read back off a ledger row
/// could be a reading another session settled an hour ago. Slots belong to
/// (project, attempt), and late results keep only their detached slot.
///
/// A recall's section is staged here until the runtime says the request that
/// carried it was answered (`heard`): a request that never left, or whose
/// answer was taken back, showed the model nothing, and its notes are shown
/// to nobody (t-6264).
#[derive(Default)]
struct Reading {
    /// The reading the label is named for when no judgment settled: the last
    /// answered recall's fingerprints, which its row is named by too.
    key: Option<(u64, u64)>,
    /// Each note an answered request's section named, once, in the order
    /// first shown. A turn recalls once per request and reads the same
    /// section each time, so this grows across the turn and one turn is one
    /// showing of each note.
    shown: Vec<ShownPath>,
    /// Unix milliseconds of the first showing.
    shown_at: Option<u64>,
    /// The vault those pages belong to, at first showing
    /// (`vault_fingerprint`).
    vault: Option<u64>,
    /// This slot's section, waiting to hear whether the request carrying it
    /// was answered.
    staged: Option<Staged>,
    /// Whether this slot's request was answered: a mark grades only a
    /// reading the model was shown.
    answered: bool,
    settled: Option<Settled>,
    /// What the turn did, as the runtime told it.
    seen: TurnSeen,
    /// Whether the runtime said the turn ended on its own terms, so that
    /// `seen` is the whole of it.
    ended: bool,
}

impl Reading {
    /// The staged section, now shown: each note once per turn, at the place
    /// it first held.
    fn show(&mut self, staged: Staged) {
        for note in staged.notes {
            if !self.shown.iter().any(|shown| shown.slug == note.slug) {
                self.shown.push(note);
            }
        }
        self.key = Some(staged.key);
        if self.shown_at.is_none() && !self.shown.is_empty() {
            self.shown_at = Some(staged.at);
            self.vault = staged.vault;
        }
    }

    /// The reading this turn's mark grades: the last one settled on a
    /// recall whose request was answered.
    fn graded(&self) -> Option<&Settled> {
        self.settled.as_ref().filter(|_| self.answered)
    }
}

/// One recall's section, before the request carrying it was answered.
struct Staged {
    key: (u64, u64),
    notes: Vec<ShownPath>,
    /// Unix milliseconds the section was built: the showing, once answered.
    at: u64,
    /// The vault its pages belong to, read only while the turn had shown
    /// nothing yet — the one showing that names it.
    vault: Option<u64>,
}

/// One note a turn was shown, as the slot remembers it: the path recall
/// handed it under, which a read is matched against, and the place it held
/// in the section at its first showing.
#[derive(Debug, Clone)]
struct ShownPath {
    slug: String,
    path: String,
    rank: usize,
}

/// The last reading settled for one attempt: the judgment's order, which the
/// seat's mark grades.
struct Settled {
    /// When the reading's row was made — the label's `requestAt`, so it
    /// grades this asking of `query` over `notes` and no other (t-6877).
    at: u64,
    query: u64,
    notes: u64,
    applied: bool,
    /// The judgment's order, each note with the path recall handed it under.
    proposed: Vec<(String, String)>,
    /// Recall's own first note, before any judgment — the seat's baseline,
    /// today's rule (`zerocode_core::jev::RECALL`, t-6342).
    recall_first: Option<(String, String)>,
}

type ReadingSlot = Arc<Mutex<Reading>>;
type ReadingBook = HashMap<(PathBuf, String), ReadingSlot>;

fn last_settled() -> &'static Mutex<ReadingBook> {
    static SETTLED: OnceLock<Mutex<ReadingBook>> = OnceLock::new();
    SETTLED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The attempt owns one slot. Replacing or ending it detaches older async
/// results: they can finish their row but cannot re-enter the label book.
/// What the turn's earlier requests showed, and what the turn did since,
/// stays with the turn, because the label counts one showing per turn, not
/// per request.
fn reading_slot(cwd: &Path, attempt: &str) -> ReadingSlot {
    let mut reading = Reading::default();
    let Ok(mut held) = last_settled().lock() else {
        return Arc::new(Mutex::new(reading));
    };
    let key = (cwd.to_path_buf(), attempt.to_string());
    if let Some(mut earlier) = held.get(&key).and_then(|slot| slot.lock().ok()) {
        reading.key = earlier.key;
        reading.shown.clone_from(&earlier.shown);
        reading.shown_at = earlier.shown_at;
        reading.vault = earlier.vault;
        reading.seen = std::mem::take(&mut earlier.seen);
    }
    let slot = Arc::new(Mutex::new(reading));
    held.insert(key, Arc::clone(&slot));
    slot
}

/// Stage what the turn is about to read: the section names the first
/// [`MAX_RECALLED_ENTRIES`] of what the seat handed back and no more
/// (`turn_support::recall_and_reminder_sections`), so a note past them, or
/// one the apply road left out, was shown to nobody. Staged and not shown:
/// the request carrying it has not left yet (`heard`).
fn note_shown(slot: &ReadingSlot, key: (u64, u64), read: &[MemoryHit]) {
    let Ok(mut reading) = slot.lock() else {
        return;
    };
    let notes = read
        .iter()
        .take(MAX_RECALLED_ENTRIES)
        .enumerate()
        .map(|(rank, hit)| ShownPath { slug: hit.entry.slug.clone(), path: hit.entry.path.clone(), rank })
        .collect();
    let vault = if reading.shown_at.is_none() { vault_fingerprint() } else { reading.vault };
    reading.staged = Some(Staged { key, notes, at: unix_millis(), vault });
}

/// What the runtime told the seat the turn did since it last heard
/// (`runtime::TurnProgress`): an answer to the request that carried the last
/// recall makes that recall's section a showing; the messages appended are
/// read here, before any compaction can take them; and a turn that ended on
/// its own terms says so.
fn heard(cwd: &Path, attempt: &str, progress: runtime::TurnProgress<'_>) {
    let slot = last_settled()
        .lock()
        .ok()
        .and_then(|held| held.get(&(cwd.to_path_buf(), attempt.to_string())).cloned());
    let Some(slot) = slot else {
        return;
    };
    let Ok(mut reading) = slot.lock() else {
        return;
    };
    let staged = reading.staged.take();
    if progress.answered {
        reading.answered = true;
        if let Some(staged) = staged {
            reading.show(staged);
        }
    }
    // A seat that has shown this turn nothing has nothing to grade and reads
    // nothing of it: an `off` recall costs no scan.
    if !reading.shown.is_empty() || reading.settled.is_some() {
        reading.seen.record(progress.appended);
    }
    reading.ended |= progress.ended;
}

/// The vault the environment names, as the fingerprint the label carries
/// and the fold filters on — read the way the retriever's corpus is read
/// (`runtime::memory::recall::load_memory_retriever`, `SecondBrain::from_env`),
/// so the two agree on which vault a page belongs to. `None` when no vault
/// is configured, which no demand ranks anything of.
fn vault_fingerprint() -> Option<u64> {
    runtime::SecondBrain::from_env().map(|vault| task_fingerprint("", &vault.root().to_string_lossy()))
}

/// Remember the order a reading settled on, when it settled on one: a row
/// whose judgment failed or could not be ordered proposes nothing, and a turn
/// that read recall's own order was not asked to agree with anything.
fn note_settled(slot: &ReadingSlot, row: &RerankShadowRow, hits: &[MemoryHit]) {
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
    let recall_first = hits.first().map(|hit| (hit.entry.slug.clone(), hit.entry.path.clone()));
    if let Ok(mut reading) = slot.lock() {
        reading.settled =
            Some(Settled { at: row.at, query: row.query, notes: row.notes, applied: row.applied, proposed, recall_first });
    }
}

/// The recall seat's `agreed` mark, one row per turn that was handed a
/// judged order: whether the note the judgment put FIRST was read or cited
/// before the turn ended — written only for a turn that touched some note it
/// was handed (`rerank_shadow::mark`, t-6342); a turn that touched none
/// carries `rerank_shadow::NO_NOTE_TOUCHED` under `notCompared` instead.
///
/// A row of its own, keyed like the reading it grades (`query`, `notes`) and
/// carrying `applied` from it, so an order the turn read and an order only
/// recorded beside recall's are compared on one mark. `rank` is the place in
/// the judgment's order of the first note the turn touched, in the order the
/// turn touched them; absent when it touched none. Shaped like the skill
/// seat's label (`skill_search::SkillLabelRow`).
///
/// Since t-6264 the same row also names every note the turn was SHOWN, once
/// each, and what became of it (`shown`) — one row per turn still, because
/// the judge counts one comparison per row that carries a mark
/// (`zerocode_core::jev::summary::agreement_rows`), and a row per note would
/// count a turn shown five notes as five turns. A turn whose reading settled
/// no judgment — the door refused it, the reply failed its checks, the
/// answer came after the turn ended — writes the row with its notes and no
/// mark: it compared nothing, but it showed something, and the demand recall
/// ranks on (`runtime::memory::recall::RecallDemand`) is read off these
/// notes alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RerankLabelRow {
    pub at: u64,
    /// The reading this row grades, spelled `<query>:<notes>` — the two
    /// fingerprints the answered row is named by.
    pub label: String,
    /// When the reading this row grades was asked — its row's time, which
    /// the settled reading carries (`zerocode_core::jev::summary::REQUEST_AT`,
    /// t-6877): the same words asked over the same notes again carry the
    /// same name, and the judge joins a label to one asking by the time.
    /// Never the showing's time (`shown_at`), which is when the model was
    /// shown the notes and not when the reading was asked. Absent on a row
    /// that grades no settled reading, and on every row from before t-6877:
    /// such a row grades no asking.
    #[serde(default, rename = "requestAt", skip_serializing_if = "Option::is_none")]
    pub request_at: Option<u64>,
    pub query: u64,
    pub notes: u64,
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<usize>,
    /// Why the row carries no `agreed`, under the summary's key
    /// (`zerocode_core::jev::summary::NOT_COMPARED`).
    #[serde(default, rename = "notCompared", skip_serializing_if = "Option::is_none")]
    pub not_compared: Option<String>,
    /// Whether recall's own first note was touched on the same turn — the
    /// seat's baseline's mark (`zerocode_core::jev::summary::BASELINE_AGREED`),
    /// written beside `agreed` (t-6342).
    #[serde(default, rename = "baselineAgreed", skip_serializing_if = "Option::is_none")]
    pub baseline_agreed: Option<bool>,
    /// Unix milliseconds of the turn's first showing — before which a replay
    /// may count nothing about these notes as known (t-6264).
    #[serde(default, rename = "shownAt", skip_serializing_if = "Option::is_none")]
    pub shown_at: Option<u64>,
    /// The vault the shown pages belong to, as a fingerprint of its root, so
    /// a demand read off this ledger folds no other vault's page of the same
    /// name. Absent when no vault was configured: the notes were the memory
    /// store's own, which no demand ranks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<u64>,
    /// Whether the turn failed after it was shown these notes, so that what
    /// the seat heard of it is not the whole of it: a note this row names as
    /// neither read nor cited is then one nobody saw the end of, not one the
    /// turn left unopened, and the demand does not count it (t-6264). Absent
    /// on a row whose turn ended on its own terms.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unfinished: bool,
    /// Every note the turn was shown, once each, in the order first shown.
    /// Empty on a row from before t-6264, which says nothing about its
    /// notes — not that none was opened.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<ShownNote>,
}

/// One note a recall put in front of a turn, and what the turn then did
/// with it (t-6264) — the per-note half of [`RerankLabelRow`].
///
/// `read` and `cited` are two observations, kept apart: a successful read
/// that named the note's own path, and a citation of the note in the
/// assistant's own words (`[[slug]]`, or its path — `own_citations`). A
/// failed read, a call that got no result, a citation the person or a tool
/// result carried, one the assistant quoted or showed as code, and a path
/// that merely ends the same way are none of them. Neither is a
/// verdict on the note: a turn may go on without opening a page that
/// answered it, and a row that says neither happened says only that. What
/// the demand reads off them is the one thing they can answer — whether
/// anyone, across many showings, ever opened the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownNote {
    pub slug: String,
    /// The note's place in the order the turn read, at its first showing.
    pub rank: usize,
    pub read: bool,
    pub cited: bool,
}

/// Write the recall seat's label for the turn that just ended, from what the
/// runtime told the seat as the turn went (`heard`) — not from the
/// transcript, which a compaction may have summarised by now. A `cancelled`
/// turn's slot is discarded without a row. A note was touched when a
/// successful read named its exact path, or the assistant's own words cited
/// its slug or path (`own_citations`). Nothing is written for a turn shown
/// nothing that has no reading it was shown to grade; answers whether a row
/// was written.
#[must_use]
pub fn note_recall_read(cwd: &Path, attempt: &str, cancelled: bool) -> bool {
    let Some(slot) = last_settled().lock().ok().and_then(|mut held| held.remove(&(cwd.to_path_buf(), attempt.to_string()))) else {
        return false;
    };
    if cancelled {
        return false;
    }
    let Some(reading) = slot.lock().ok().map(|mut held| std::mem::take(&mut *held)) else {
        return false;
    };
    // A seat that asked nothing, or whose recalls never reached the model,
    // showed nothing: there is no row to write about that.
    if reading.shown.is_empty() && reading.graded().is_none() {
        return false;
    }
    let row = label_row(&reading);
    let ledger = rerank_shadow_path(cwd);
    let written = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES).is_ok();
    // A label may be the mark that clears the seat's agreement line: judge
    // now rather than at the next reading's write, so a rise the labels
    // earned is read by the very next recall — off the turn that is ending.
    judge_detached(ledger);
    written
}

/// The word a recall label carries under the summary's `notCompared` when
/// the turn read and cited none of the notes it was handed: the order was
/// never compared with anything (t-6342).
pub const NO_NOTE_TOUCHED: &str = "no_note_touched";

/// The recall seat's mark for one turn (t-6342): whether the note the
/// judgment put first was among the notes the turn touched — counted only for
/// a turn that touched one. `touched` is the judgment's ranks of the notes
/// the turn touched, in the order it touched them.
///
/// # Errors
///
/// [`NO_NOTE_TOUCHED`] for a turn that read and cited none of them: the order
/// was compared with nothing, and on this machine that was 82 of the 88 turns
/// the seat had been graded on.
pub fn mark(touched: &[usize]) -> Result<bool, &'static str> {
    if touched.is_empty() {
        Err(NO_NOTE_TOUCHED)
    } else {
        Ok(touched.contains(&0))
    }
}

/// The label row itself: every note shown and what became of it, and — for
/// a reading whose judgment settled on a recall the model was shown — the
/// mark, or why there is none, and the rank of the first note touched.
fn label_row(reading: &Reading) -> RerankLabelRow {
    let touched = &reading.seen.touched;
    let named: Vec<(String, String)> = reading.shown.iter().map(|shown| (shown.slug.clone(), shown.path.clone())).collect();
    let shown = reading
        .shown
        .iter()
        .zip(seen_of(&named, touched))
        .map(|(shown, seen)| ShownNote { slug: shown.slug.clone(), rank: shown.rank, read: seen.read, cited: seen.cited })
        .collect();
    let graded = reading.graded();
    let (query, notes) = graded.map_or(reading.key.unwrap_or_default(), |settled| (settled.query, settled.notes));
    let mut row = RerankLabelRow {
        at: unix_millis(),
        label: format!("{query}:{notes}"),
        request_at: graded.map(|settled| settled.at),
        query,
        notes,
        applied: false,
        agreed: None,
        rank: None,
        not_compared: None,
        baseline_agreed: None,
        shown_at: reading.shown_at,
        vault: reading.vault,
        unfinished: !reading.ended,
        shown,
    };
    if let Some(settled) = graded {
        let first_touched: Vec<usize> = touched_in_order(&settled.proposed, touched);
        let marked = mark(&first_touched);
        row.applied = settled.applied;
        row.agreed = marked.ok();
        row.rank = first_touched.first().copied();
        row.not_compared = marked.err().map(str::to_string);
        // Today's rule on the same turn: whether recall's own first note was
        // touched — marked only beside a mark of the seat's, so the two are
        // read over the same turns.
        row.baseline_agreed = marked.is_ok().then(|| {
            settled
                .recall_first
                .as_ref()
                .is_some_and(|first| !touched_in_order(std::slice::from_ref(first), touched).is_empty())
        });
    }
    row
}

/// What a turn did with one note it was handed: the two observations, and
/// the order in which the note was first touched by either.
#[derive(Debug, Clone, Copy, Default)]
struct Seen {
    read: bool,
    cited: bool,
    touched_at: Option<usize>,
}

/// What a turn did, as far as the seat reads it: each read whose result came
/// back without an error, and each target the assistant's own words cited,
/// in the order they happened, each once — recorded as the runtime hands the
/// turn over, so a compaction that later summarises the transcript cannot
/// take it back.
#[derive(Debug, Default)]
struct TurnSeen {
    /// Reads whose result has not come back yet: tool-use id → the path.
    reading: HashMap<String, String>,
    touched: Vec<Touch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Touch {
    /// A successful read, by the path it named.
    Read(String),
    /// A target the assistant's own words cited ([`own_citations`]).
    Cited(String),
}

impl TurnSeen {
    /// Read what `appended` shows the turn doing. A read is the assistant's
    /// call, and counts once its own result came back without an error; a
    /// citation is the assistant's own words. What the person typed and what
    /// a tool returned are neither.
    fn record(&mut self, appended: &[ConversationMessage]) {
        for message in appended {
            for block in &message.blocks {
                match block {
                    ContentBlock::ToolUse { id, name, input } if message.role == MessageRole::Assistant => {
                        if let Some(path) = read_path(name, input) {
                            self.reading.insert(id.clone(), path);
                        }
                    }
                    ContentBlock::ToolResult { tool_use_id, is_error, .. } if message.role == MessageRole::Tool => {
                        if let Some(path) = self.reading.remove(tool_use_id.as_str()) {
                            if !is_error {
                                self.touch(Touch::Read(path));
                            }
                        }
                    }
                    ContentBlock::Text { text } if message.role == MessageRole::Assistant => {
                        for target in own_citations(text) {
                            self.touch(Touch::Cited(target));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn touch(&mut self, touch: Touch) {
        if !self.touched.contains(&touch) {
            self.touched.push(touch);
        }
    }
}

/// The targets the assistant's own words cite in `text`
/// (`decision_core::dreamer::cited_targets`), outside what it quotes and
/// what it shows as code (t-6264). The answer is read as `CommonMark`, by the
/// parser the window's renderer draws it with (`pulldown_cmark`), so a block
/// quote carries someone else's words to its end and not to its last `>`:
/// a line that carries the quoted paragraph on without one — a lazy
/// continuation — is quoted too, and so is a quote inside the quote and a
/// fence the quote holds; a code block, fenced or indented, shows code or
/// output. A link or a path in either is not the assistant naming the page.
/// Each such block's source is set aside before any target is read, so
/// nothing quoted is ever joined to the assistant's own words.
fn own_citations(text: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Parser, Tag};
    let mut own = String::with_capacity(text.len());
    let mut from = 0;
    for (event, block) in Parser::new(text).into_offset_iter() {
        if block.start >= from && matches!(event, Event::Start(Tag::BlockQuote(_) | Tag::CodeBlock(_))) {
            own.push_str(text.get(from..block.start).unwrap_or_default());
            own.push('\n');
            from = block.end;
        }
    }
    own.push_str(text.get(from..).unwrap_or_default());
    decision_core::dreamer::cited_targets(&own)
}

/// The ranks of the notes a turn touched, in the order it touched them, each
/// once.
fn touched_in_order(notes: &[(String, String)], touched: &[Touch]) -> Vec<usize> {
    let mut first: Vec<(usize, usize)> = seen_of(notes, touched)
        .iter()
        .enumerate()
        .filter_map(|(rank, seen)| seen.touched_at.map(|at| (at, rank)))
        .collect();
    first.sort_unstable();
    first.into_iter().map(|(_, rank)| rank).collect()
}

/// What the turn did with each of `notes`, one [`Seen`] per note in the
/// notes' own order: read when a successful read named its exact path, cited
/// when the assistant's own words named its slug or path (`cites`) — the one
/// reading of what the turn touched, so the mark and the per-note
/// observations cannot disagree about a turn.
fn seen_of(notes: &[(String, String)], touched: &[Touch]) -> Vec<Seen> {
    let mut seen = vec![Seen::default(); notes.len()];
    let mut order = 0usize;
    for event in touched {
        for (rank, (slug, path)) in notes.iter().enumerate() {
            let read = match event {
                Touch::Read(read) if !path.is_empty() && path == read => true,
                Touch::Cited(target) if cites(target, slug, path) => false,
                _ => continue,
            };
            touch(&mut seen, &mut order, rank, read);
        }
    }
    seen
}

fn touch(seen: &mut [Seen], order: &mut usize, rank: usize, read: bool) {
    let note = &mut seen[rank];
    if read {
        note.read = true;
    } else {
        note.cited = true;
    }
    if note.touched_at.is_none() {
        note.touched_at = Some(*order);
        *order += 1;
    }
}

/// Whether a cited target names a note: its slug, or its path whole or by a
/// tail that begins at a path component.
fn cites(target: &str, slug: &str, path: &str) -> bool {
    target == slug || target == path || path.ends_with(&format!("/{target}"))
}

/* ---- the demand: what the rows say readers did, read back by recall ------ */

impl RecallDemandSource for RerankShadow {
    fn demand(&self) -> Option<Arc<RecallDemand>> {
        demand_for(&self.cwd)
    }
}

/// The demand recall ranks this project's next recall on — what this seat's
/// own rows say readers did with the pages recall kept showing them — or
/// `None` on a road that records only: `off`, `shadow`, and an `auto` its
/// evidence has not raised (`seat_acts`). Under those the retriever ranks
/// as it always did, byte for byte, which is what a record-only mode
/// promises; the rows still accrue, so the day the seat rises it rises with
/// its readers' answers in hand.
///
/// Read per recall, as the seat reads its mode per recall: a label the last
/// turn wrote is folded into this one's demand, and a switch a person flips
/// takes effect on the next recall. The fold is kept up to the byte
/// ([`FoldedLedger`]), so a recall parses the rows written since the last
/// one and not the ledger — though one whose ledger changed reads the rest
/// again, to hold it to what was folded; and the fold is asked before the
/// settings, because a demand that sinks no page ranks exactly as none
/// does — until a page has been left unopened five times, a recall whose
/// ledger did not change pays one look at it here ([`LedgerLook`]) and
/// reads no settings at all.
///
/// An ablation (`telemetry::attest_ablated`) is not asked here: it holds the
/// JUDGMENT out, on the road that asks one, and records the holding-out per
/// recall as it goes; a bench that wants the demand held out too holds the
/// seat at `shadow`, which is the one word that does both.
fn demand_for(cwd: &Path) -> Option<Arc<RecallDemand>> {
    let demand = {
        let mut book = demand_book().lock().ok()?;
        let ledger = rerank_shadow_path(cwd);
        book.entry(ledger.clone()).or_default().catch_up(&ledger, vault_fingerprint())
    };
    if !demand.sinks_anything() {
        return None;
    }
    let mode = rerank_shadow_mode_from(&runtime::ConfigLoader::default_for(cwd))?;
    seat_acts(cwd, mode).then_some(demand)
}

/// This process's fold of one ledger's label rows into recall's demand,
/// kept up to the byte it has folded, with a fingerprint of every window of
/// the bytes it folded.
#[derive(Debug, Default)]
struct FoldedLedger {
    /// What the last look at the ledger saw: a look that sees the same has
    /// nothing new to fold.
    seen: Option<LedgerLook>,
    /// The end of the last whole line folded: the next fold starts here.
    folded_to: u64,
    /// A fingerprint of each [`FOLD_WINDOW_BYTES`] of the bytes folded,
    /// counted from the ledger's start — the last of them, of what lies past
    /// the last whole window.
    windows: Vec<u64>,
    /// The vault the fold was made for; another vault starts the fold over.
    vault: Option<u64>,
    /// `slug → (times shown, times opened)`, across every row folded.
    tally: BTreeMap<String, (u32, u32)>,
    demand: Arc<RecallDemand>,
}

/// How many of a ledger's bytes one of its fold's fingerprints covers
/// ([`FoldedLedger::windows`]): the unit the fold reads what it folded
/// again in, sixty-four of them at the ledger's cap
/// ([`SHADOW_LEDGER_MAX_BYTES`]) — one read each, and a rewrite is found at
/// the first window it touched.
const FOLD_WINDOW_BYTES: u64 = SHADOW_LEDGER_MAX_BYTES / 64;

fn demand_book() -> &'static Mutex<HashMap<PathBuf, FoldedLedger>> {
    static BOOK: OnceLock<Mutex<HashMap<PathBuf, FoldedLedger>>> = OnceLock::new();
    BOOK.get_or_init(|| Mutex::new(HashMap::new()))
}

impl FoldedLedger {
    /// Fold the rows written since the last fold, and answer the demand.
    ///
    /// Everything is read through one handle, so a ledger replaced between
    /// two reads is never read as half of each. A ledger this look sees as
    /// it was last seen ([`LedgerLook`]) has nothing new. Any other is held
    /// to what the fold read of it (t-6264): the same file by its birth, no
    /// shorter, and every window the fold read still the bytes it read —
    /// then what lies past the fold is folded on from there. One that is
    /// not — cut to its newer half (`shadow_ledger::append_shadow_row`),
    /// replaced, rewritten in place under the same name and birth, at the
    /// length it had or then grown past it — and one asked for another
    /// vault is folded again from its start. Only whole lines are folded: a
    /// row being appended as this reads is left for the next fold, which
    /// starts where this one stopped.
    fn catch_up(&mut self, ledger: &Path, vault: Option<u64>) -> Arc<RecallDemand> {
        let looked = fs::File::open(ledger).ok().and_then(|mut file| LedgerLook::take(&mut file).map(|look| (file, look)));
        let Some((mut file, now)) = looked else {
            // No ledger that can be read: no row says anything.
            *self = Self { vault, ..Self::default() };
            return Arc::clone(&self.demand);
        };
        if self.vault == vault && self.seen.as_ref() == Some(&now) {
            return Arc::clone(&self.demand);
        }
        let same_file = self.vault == vault
            && now.len >= self.folded_to
            && self.seen.as_ref().is_some_and(|seen| seen.created == now.created);
        let held = if same_file { self.still_held(&mut file) } else { None };
        let rest = held.unwrap_or_else(|| {
            *self = Self { vault, ..Self::default() };
            Vec::new()
        });
        // What lies past the fold, up to what this look saw: a row appended
        // since is the next look's.
        let Some(tail) = bytes_at(&mut file, self.folded_to, now.len - self.folded_to) else {
            return Arc::clone(&self.demand);
        };
        let whole = tail.iter().rposition(|byte| *byte == b'\n').map_or(0, |at| at + 1);
        for line in String::from_utf8_lossy(&tail[..whole]).lines() {
            let Ok(row) = serde_json::from_str::<RerankLabelRow>(line) else {
                continue;
            };
            fold_shown(&mut self.tally, &row, vault);
        }
        self.count(&rest, &tail[..whole]);
        self.folded_to += u64::try_from(whole).unwrap_or(u64::MAX);
        self.seen = Some(now);
        self.demand = Arc::new(RecallDemand::from_rows(
            self.tally.iter().map(|(slug, (recalled, opened))| (slug.clone(), *recalled, *opened)),
        ));
        Arc::clone(&self.demand)
    }

    /// Whether every window the fold read is, in `file`, the bytes it read
    /// — each one read again and its fingerprint compared, the first that
    /// differs ending the look — and if so the bytes past the last whole
    /// window, which the next rows fill on.
    fn still_held(&self, file: &mut fs::File) -> Option<Vec<u8>> {
        let mut rest = Vec::new();
        for (start, counted) in (0..).step_by(usize::try_from(FOLD_WINDOW_BYTES).ok()?).zip(&self.windows) {
            let bytes = bytes_at(file, start, FOLD_WINDOW_BYTES.min(self.folded_to - start))?;
            if window_fingerprint(&[&bytes]) != *counted {
                return None;
            }
            rest = bytes;
        }
        if self.folded_to.is_multiple_of(FOLD_WINDOW_BYTES) {
            rest.clear();
        }
        Some(rest)
    }

    /// Fingerprint the windows `folded` fills and starts, `rest` being the
    /// bytes already in the window it goes on from.
    fn count(&mut self, rest: &[u8], folded: &[u8]) {
        let window = usize::try_from(FOLD_WINDOW_BYTES).unwrap_or(usize::MAX);
        self.windows.truncate(usize::try_from(self.folded_to / FOLD_WINDOW_BYTES).unwrap_or(usize::MAX));
        let (filling, mut after) = folded.split_at(window.saturating_sub(rest.len()).min(folded.len()));
        if !rest.is_empty() || !filling.is_empty() {
            self.windows.push(window_fingerprint(&[rest, filling]));
        }
        while !after.is_empty() {
            let (next, more) = after.split_at(window.min(after.len()));
            self.windows.push(window_fingerprint(&[next]));
            after = more;
        }
    }
}

/// A fingerprint of one window of a ledger's bytes, given in `parts` — the
/// same as of the parts run together.
fn window_fingerprint(parts: &[&[u8]]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::hash::DefaultHasher::new();
    for part in parts {
        hasher.write(part);
    }
    hasher.finish()
}

/// What one look at a ledger sees, through one open handle: its length, when
/// it was last written and when made, and the first and last row's worth of
/// its bytes ([`TAIL_ROW_BYTES`], the most a row takes). Two looks that
/// agree on all of it are taken to be one state of one file, and a length
/// alone is never: a ledger replaced by another of the very same length —
/// the same first bytes, even — differs in when it was written or in its
/// last row (t-6264). Any look that differs has the fold read what it
/// folded again ([`FoldedLedger::catch_up`]) and the standing read afresh
/// (`raised_now`). What a look cannot tell from the state it saw is a
/// rewrite in place of the very length that kept both end rows byte for
/// byte and left the file's clock where it stood — written inside one of
/// its ticks, or with the clock put back; nothing zo runs writes a ledger
/// that way.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerLook {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    first: Vec<u8>,
    last: Vec<u8>,
}

impl LedgerLook {
    fn take(file: &mut fs::File) -> Option<Self> {
        let meta = file.metadata().ok()?;
        let len = meta.len();
        Some(Self {
            len,
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            first: bytes_at(file, 0, len.min(TAIL_ROW_BYTES))?,
            last: row_ending_at(file, len)?,
        })
    }

    /// One look at the ledger at `path`; `None` when there is none to open.
    fn of(path: &Path) -> Option<Self> {
        let mut file = fs::File::open(path).ok()?;
        Self::take(&mut file)
    }
}

/// The row's worth of `file`'s bytes that ends at `end`.
fn row_ending_at(file: &mut fs::File, end: u64) -> Option<Vec<u8>> {
    let width = end.min(TAIL_ROW_BYTES);
    bytes_at(file, end - width, width)
}

/// `width` of `file`'s bytes from `from`, or fewer where the file ends first.
fn bytes_at(file: &mut fs::File, from: u64, width: u64) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::with_capacity(usize::try_from(width).ok()?);
    file.by_ref().take(width).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Fold one label row's showings into `tally`: each note the row names once,
/// shown once and opened once when either observation says so. A row
/// written under another vault — or under none, when the notes were the
/// memory store's alone — is not this vault's evidence (`vault`); a row from
/// before t-6264 names no notes and adds nothing — it does not say they went
/// unopened; and an `unfinished` row's note that neither observation names
/// is unknown, not unopened, and adds nothing either.
fn fold_shown(tally: &mut BTreeMap<String, (u32, u32)>, row: &RerankLabelRow, vault: Option<u64>) {
    if row.vault != vault {
        return;
    }
    let mut named: Vec<&str> = Vec::with_capacity(row.shown.len());
    for note in &row.shown {
        let opened = note.read || note.cited;
        if note.slug.is_empty() || named.contains(&note.slug.as_str()) || (row.unfinished && !opened) {
            continue;
        }
        named.push(&note.slug);
        let counted = tally.entry(note.slug.clone()).or_default();
        counted.0 = counted.0.saturating_add(1);
        counted.1 = counted.1.saturating_add(u32::from(opened));
    }
}

/// The demand `rows` — this seat's label rows — hold for `vault`, folded as
/// [`FoldedLedger`] folds them: one showing per row a note appears on, one
/// opening when the row says it was read or cited. What a replay or a
/// counter reads; the seat itself folds incrementally.
#[cfg(test)]
#[must_use]
pub fn recall_demand_from<'a>(rows: impl IntoIterator<Item = &'a RerankLabelRow>, vault: Option<u64>) -> RecallDemand {
    let mut tally = BTreeMap::new();
    for row in rows {
        fold_shown(&mut tally, row, vault);
    }
    RecallDemand::from_rows(tally.into_iter().map(|(slug, (recalled, opened))| (slug, recalled, opened)))
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
    let key = MemoKey::for_reading(query, hits, door.model_key());
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
    use std::sync::Arc;

    use runtime::memory::rerank::RERANK_LEVELS;
    use runtime::MemoryEntry;

    use super::super::jev_mock::Mock;
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

    /// The workspace every reading here comes from, consented at a door that
    /// reads no machine's settings.
    const WORKSPACE: &str = "/work/zo";

    fn door(consented: &[&str], home: &Path) -> JevDoor {
        let settings = zerocode_core::jev::door::JevSettings {
            enabled: true,
            workspaces: consented.iter().map(|root| (*root).to_string()).collect(),
            daily_requests: None,
            model: zerocode_core::jev::DEFAULT_MODEL.to_string(),
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
            model: zerocode_core::jev::DEFAULT_MODEL.to_string(),
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
                MemoKey::for_reading("a past reading", &[], jev_gate::model_key(SYSTEMONE_MODEL)),
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
    /// workspace and the recall switch set to `mode`, a key, and a mock origin
    /// — the shared machine (`jev_mock`), told which seat's switch to set.
    fn machine<T>(mode: &str, base_url: &str, body: impl FnOnce(&Path) -> T) -> T {
        super::super::jev_mock::machine(&RECALL, mode, base_url, body)
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
            MemoKey::for_reading("anything", &hits, jev_gate::model_key(SYSTEMONE_MODEL)),
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
    const TEST_ATTEMPT: &str = "session@test-turn";

    fn settle(cwd: &Path, query: &str, hits: Vec<MemoryHit>) -> Vec<MemoryHit> {
        super::settle(cwd, TEST_ATTEMPT, query, hits)
    }

    /// A turn whose one request carried the recall and was answered, as the
    /// runtime tells it: what the turn appended, handed over at its end, and
    /// then its label.
    fn note_recall_read(cwd: &Path, turn: &[ConversationMessage]) -> bool {
        heard(cwd, TEST_ATTEMPT, told(turn, true, true));
        super::note_recall_read(cwd, TEST_ATTEMPT, false)
    }

    /// What the runtime hands the seat at a boundary.
    fn told(appended: &[ConversationMessage], answered: bool, ended: bool) -> runtime::TurnProgress<'_> {
        runtime::TurnProgress { appended, answered, ended }
    }

    /// What a turn that appended `messages` touched, as the seat records it.
    fn touched(messages: &[ConversationMessage]) -> Vec<Touch> {
        let mut seen = TurnSeen::default();
        seen.record(messages);
        seen.touched
    }

    fn turn(blocks: Vec<ContentBlock>) -> Vec<ConversationMessage> {
        let mut messages = vec![ConversationMessage::user_text("the question")];
        for block in blocks {
            let result = match &block {
                ContentBlock::ToolUse { id, name, .. } => Some(ConversationMessage::tool_result(id, name, "read completed", false)),
                _ => None,
            };
            messages.push(ConversationMessage::assistant(vec![block]));
            messages.extend(result);
        }
        messages
    }

    fn read_of(path: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: "toolu_1".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"path": path}).to_string(),
        }
    }

    #[test]
    fn editing_a_proposed_note_is_not_evidence_that_the_turn_read_it() {
        let proposed = vec![("wiki/note".to_string(), "/vault/wiki/note.md".to_string())];
        let edit = ContentBlock::ToolUse {
            id: "toolu_edit".to_string(),
            name: "Edit".to_string(),
            input: serde_json::json!({"file_path": proposed[0].1}).to_string(),
        };
        assert!(touched_in_order(&proposed, &touched(&turn(vec![edit]))).is_empty());
    }

    #[test]
    fn a_failed_read_does_not_label_the_note_as_read() {
        let proposed = vec![("wiki/note".to_string(), "/vault/wiki/note.md".to_string())];
        let messages = vec![
            ConversationMessage::assistant(vec![read_of(&proposed[0].1)]),
            ConversationMessage::tool_result("toolu_1", crate::file_tools::READ_FILE_TOOL_NAME, "read denied", true),
        ];
        assert!(touched_in_order(&proposed, &touched(&messages)).is_empty());
    }

    #[test]
    fn two_attempts_in_one_project_do_not_take_each_others_reading() {
        let work = tempfile::tempdir().expect("workspace");
        let first = reading_slot(work.path(), "first");
        let second = reading_slot(work.path(), "second");
        assert!(!super::note_recall_read(work.path(), "first", true));
        let held = last_settled().lock().expect("book");
        let remaining = held.get(&(work.path().to_path_buf(), "second".to_string())).expect("second attempt");
        assert!(Arc::ptr_eq(remaining, &second));
        assert!(!Arc::ptr_eq(remaining, &first));
        drop(held);
        assert!(!super::note_recall_read(work.path(), "second", true));
    }

    #[test]
    fn a_tool_call_without_a_successful_result_did_not_read_the_note() {
        let proposed = vec![("wiki/note".to_string(), "/vault/wiki/note.md".to_string())];
        let messages = vec![ConversationMessage::assistant(vec![read_of(&proposed[0].1)])];
        assert!(touched_in_order(&proposed, &touched(&messages)).is_empty());
    }

    #[test]
    fn an_ended_turn_cannot_receive_a_reading_that_settles_later() {
        let work = tempfile::tempdir().expect("workspace");
        let slot = reading_slot(work.path(), TEST_ATTEMPT);
        assert!(!note_recall_read(work.path(), &[]));
        let hits = three();
        let mut row = RerankShadowRow::new(
            MemoKey::for_reading("late reading", &hits, jev_gate::model_key(SYSTEMONE_MODEL)),
            hits.len(),
            RERANK_OUTCOME_ANSWERED.to_string(),
        );
        row.judged = Some(Judged {
            recalled: vec![hits[0].entry.slug.clone()],
            proposed: vec![hits[0].entry.slug.clone()],
            moved: 0, top_changed: false, held_by_graph: Vec::new(),
            dropped: Vec::new(), kept_by_graph: Vec::new(), readings: Vec::new(),
        });
        note_settled(&slot, &row, &hits);
        assert!(!last_settled().lock().expect("book").contains_key(&(work.path().to_path_buf(), TEST_ATTEMPT.to_string())));
    }

    #[test]
    fn reading_a_path_with_the_note_as_a_prefix_does_not_read_the_note() {
        let proposed = vec![("wiki/note".to_string(), "/vault/wiki/note.md".to_string())];
        assert!(touched_in_order(
            &proposed,
            &touched(&turn(vec![read_of("/vault/wiki/note.md.bak")]))
        ).is_empty());
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
        assert_eq!((label.agreed, label.rank, label.applied), (Some(true), Some(0), true));
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
        assert_eq!((labeled[0].agreed, labeled[0].rank), (Some(false), Some(1)), "{labeled:?}");
    }

    /// A turn that read and cited none of the notes it was handed compared no
    /// order at all (t-6342): 82 of the 88 marks this machine's ledger held
    /// were such turns, so the seat's agreement counted how often recall went
    /// unused rather than how often its order was right. The row stays — the
    /// reading was graded, and "nothing was touched" is a fact a reader counts
    /// — with no mark and the reason under the summary's key.
    #[test]
    fn a_turn_that_touched_no_note_is_not_a_comparison() {
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
        assert_eq!(
            (labeled[0].agreed, labeled[0].rank, labeled[0].not_compared.as_deref()),
            (None, None, Some(NO_NOTE_TOUCHED)),
            "{labeled:?}"
        );
        // Written under the key every labeled seat's reader counts.
        let written = serde_json::to_value(&labeled[0]).expect("a row");
        assert_eq!(
            written[zerocode_core::jev::summary::NOT_COMPARED.canonical],
            serde_json::json!(NO_NOTE_TOUCHED)
        );
        assert!(written.get(zerocode_core::jev::summary::AGREED.canonical).is_none(), "{written}");
        // And the judge counts nothing for it.
        let rows = vec![written];
        assert_eq!(zerocode_core::jev::summary::agreement_since(&rows, 0).compared, 0);
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
        assert_eq!((labeled[0].agreed, labeled[0].rank, labeled[0].applied), (Some(true), Some(0), false));
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
        use zerocode_core::jev::promote::{marks_that_can_clear, window_wanted_for, Verdict, ROSE};
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
            // The seat's own width, which is also its first judgment
            // boundary: the cadence counts from the row the window can first
            // be full on.
            let wanted = window_wanted_for(&RECALL).expect("recall rises");
            let already = rows(cwd).len();
            for at in 0..(wanted - already) {
                let row = serde_json::json!({
                    "at": 1_000 + at, "query": at, "notes": at, "rubric_version": RERANK_RUBRIC_VERSION,
                    "outcome": RERANK_OUTCOME_ANSWERED, "candidates": 3, "elapsed_ms": 300, "retries": 0,
                    "requests": 1, "redactedLines": 0, "applied": false,
                });
                append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES).expect("a reading");
            }
            // Dated after the window's first reading — the real one `settle`
            // wrote on this clock — because the judge reads only the marks
            // written since the window began; a label dated before it is a
            // label about some other window (t-6155 F1 made this visible:
            // until then a hindsight seat rose with no marks counted at all).
            let after_the_window = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.as_millis())
                    .unwrap_or_default(),
            )
            .unwrap_or(i64::MAX / 2)
                + 60_000;
            // Enough turns to bound above the budget with the three the label
            // said no to inside, and recall's own first note beside each
            // (t-6342).
            let misses = RECALL.negatives_wanted.expect("recall rises");
            let marks = marks_that_can_clear(&RECALL).expect("recall rises");
            for at in 0..marks {
                // Named by the words and the time the reading was made, as
                // the seat's label writer names it (t-6877).
                let label = serde_json::json!({
                    "at": after_the_window + i64::try_from(at).unwrap_or_default(), "label": format!("{at}:{at}"),
                    "requestAt": 1_000 + at, "query": at, "notes": at,
                    "applied": false, "agreed": at >= misses, "baselineAgreed": at % 2 == 0, "rank": 0,
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

    /* ---- what the turn was shown, note by note (t-6264) ------------------ */

    /// A vault whose hub three pages cite — the shape of
    /// `runtime::memory::recall`'s own demand test, written to disk so the
    /// production loader scans it the way a session's is scanned.
    fn vault_with_a_hub(dir: &Path) -> PathBuf {
        let vault = dir.join("vault");
        let wiki = vault.join("wiki");
        std::fs::create_dir_all(&wiki).expect("a wiki");
        let page = |name: &str, body: &str| {
            std::fs::write(wiki.join(format!("{name}.md")), body).expect("a page");
        };
        page("seed", "---\ntitle: seed\nrelated: [[[wiki/a-quiet]], [[wiki/z-hub]]]\n---\n\nvellichor\n");
        page("a-quiet", "---\ntitle: a-quiet\n---\n\nsonder\n");
        page("z-hub", "---\ntitle: z-hub\n---\n\nhiraeth\n");
        for cite in ["cite-1", "cite-2", "cite-3"] {
            page(cite, &format!("---\ntitle: {cite}\nrelated: [[wiki/z-hub]]\n---\n\nkomorebi\n"));
        }
        vault
    }

    /// The vault the environment names, for the body of a `machine` — which
    /// holds the crate's environment lock already, so this restores the word
    /// itself rather than taking a second guard.
    struct VaultEnv(Option<std::ffi::OsString>);

    impl VaultEnv {
        fn point_at(vault: &Path) -> Self {
            let previous = std::env::var_os(runtime::second_brain::VAULT_ENV);
            std::env::set_var(runtime::second_brain::VAULT_ENV, vault);
            Self(previous)
        }
    }

    impl Drop for VaultEnv {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var(runtime::second_brain::VAULT_ENV, previous),
                None => std::env::remove_var(runtime::second_brain::VAULT_ENV),
            }
        }
    }

    /// Every row of this project's ledger as the reader sees it.
    fn values(cwd: &Path) -> Vec<serde_json::Value> {
        super::super::shadow_ledger::read_shadow_rows(&rerank_shadow_path(cwd))
    }

    /// The label row names each note the turn was shown, in the order it
    /// read them, and says of each whether a successful read named its path
    /// and whether the assistant's own words cited it — two observations,
    /// kept apart. A read of some other file, and a citation the person
    /// typed, are neither.
    #[test]
    fn a_label_names_each_note_the_turn_was_shown_and_what_the_turn_did_with_it() {
        let mock = Mock::serving(200, reply_for(&[1, 3, 2]));
        let hits = three();
        let rows = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (shown, on)", hits.to_vec());
            assert_eq!(slugs(&read), ["wiki/b", "wiki/c", "wiki/a"], "the judgment's order is what the turn read");
            let c = read[1].entry.path.clone();
            let mut messages = turn(vec![read_of(&c), read_of("/somewhere/else.md"), said("see [[wiki/a]]")]);
            messages.push(ConversationMessage::user_text("[[wiki/b]] typed by the person"));
            assert!(note_recall_read(cwd, &messages));
            values(cwd)
        });
        let label = rows.iter().find(|row| row.get("label").is_some()).expect("a label row");
        assert_eq!(
            label["shown"],
            serde_json::json!([
                {"slug": "wiki/b", "rank": 0, "read": false, "cited": false},
                {"slug": "wiki/c", "rank": 1, "read": true, "cited": false},
                {"slug": "wiki/a", "rank": 2, "read": false, "cited": true},
            ]),
            "{label}"
        );
        assert!(label["shownAt"].as_u64().is_some_and(|at| at > 0), "{label}");
        // And the mark the judge reads is what it was: the first note was
        // not touched, the first touched sat second.
        assert_eq!((&label["agreed"], &label["rank"]), (&serde_json::json!(false), &serde_json::json!(1)));
    }

    /// A reading whose judgment never settled — the reply broke the contract
    /// — still put notes in front of the turn, and the row says which and
    /// what became of them; it carries no mark, because there was no order
    /// to compare, and the judge counts nothing for it.
    #[test]
    fn a_reading_with_no_judgment_still_says_what_the_turn_was_shown() {
        let mock = Mock::serving(200, "{\"not\": \"a reply\"}".to_string());
        let hits = three();
        let rows = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (shown, no judgment)", hits.to_vec());
            assert_eq!(slugs(&read), slugs(&hits), "recall's order stands when the reply fails its checks");
            assert!(note_recall_read(cwd, &turn(vec![read_of(&read[0].entry.path)])), "no row was written");
            values(cwd)
        });
        let label = rows.iter().find(|row| row.get("label").is_some()).expect("a label row");
        assert_eq!(
            label["shown"],
            serde_json::json!([
                {"slug": "wiki/a", "rank": 0, "read": true, "cited": false},
                {"slug": "wiki/b", "rank": 1, "read": false, "cited": false},
                {"slug": "wiki/c", "rank": 2, "read": false, "cited": false},
            ]),
            "{label}"
        );
        for key in [
            zerocode_core::jev::summary::AGREED.canonical,
            zerocode_core::jev::summary::NOT_COMPARED.canonical,
            zerocode_core::jev::summary::BASELINE_AGREED.canonical,
        ] {
            assert!(label.get(key).is_none(), "{key} on a row with no judgment: {label}");
        }
        let agreement = zerocode_core::jev::summary::agreement_since(&rows, 0);
        assert_eq!((agreement.compared, agreement.not_compared), (0, 0), "the judge counted a showing as a comparison");
        // And every other counter reads the reading's row alone: the window,
        // the cadence, and the version a row names.
        let readings: Vec<serde_json::Value> = rows.iter().filter(|row| row.get("label").is_none()).cloned().collect();
        assert_eq!(readings.len(), 1, "{rows:?}");
        assert_eq!(
            zerocode_core::jev::summary::summarize(&rows, 0),
            zerocode_core::jev::summary::summarize(&readings, 0),
            "the tally counted a label"
        );
        assert_eq!(
            zerocode_core::jev::promote::asked_toward_judgment(&RECALL, &rows),
            zerocode_core::jev::promote::asked_toward_judgment(&RECALL, &readings),
            "the cadence counted a label"
        );
        assert!(!rows.iter().filter(|row| row.get("label").is_some()).any(zerocode_core::jev::summary::is_request_or_mark));
    }

    /// The label names the asking it grades by that asking's own time
    /// (`requestAt`, t-6877) — the time the row of the reading it grades was
    /// made — and never by the turn's first showing (`shownAt`, t-6264),
    /// which is when the model was shown a section and not when the graded
    /// reading was asked (astra m-8636). A turn whose first request showed a
    /// reading of other words, and whose second asked the words it is graded
    /// on, holds the two times apart; an earlier turn asked those words over
    /// the same notes too, so two askings carry the label's name. The common
    /// reader (`promote::on_the_newest_version`) joins the label to the one
    /// asking at its time, and the judge compares it once. A label named by
    /// the showing's time would name no asking, and grade nothing.
    #[test]
    fn a_label_names_the_asking_it_grades_by_that_askings_time_and_not_the_showings() {
        let mock = Mock::serving(200, reply_for(&[1, 3, 2]));
        let words = "which note answers this (asked twice, graded once)";
        // Every clock this test reads moves on a millisecond between the
        // steps, so no two of the times it compares can meet by accident.
        let tick = || std::thread::sleep(Duration::from_millis(2));
        let (values, labels, readings) = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            // The earlier turn: the same words over the same notes — the same
            // name, another asking. It ends cancelled, and labels nothing.
            let earlier = "session@earlier-turn";
            let _read = super::settle(cwd, earlier, words, three().to_vec());
            assert!(!super::note_recall_read(cwd, earlier, true), "a cancelled turn wrote a label");
            tick();
            // This turn's first request shows a reading of other words over
            // the notes in another order, and is answered.
            let mut other = three().to_vec();
            other.reverse();
            let _first = settle(cwd, "which note answers this (shown first)", other);
            heard(cwd, TEST_ATTEMPT, told(&[], true, false));
            tick();
            // Its second asks the words the turn is graded on, after that
            // showing, and the turn reads the note the judgment put first.
            let second = settle(cwd, words, three().to_vec());
            assert!(note_recall_read(cwd, &turn(vec![read_of(&second[0].entry.path)])));
            (values(cwd), labels(cwd), rows(cwd))
        });
        let [label] = labels.as_slice() else {
            panic!("one turn, one label: {labels:?}");
        };
        let named = values
            .iter()
            .find(|row| row.get(zerocode_core::jev::summary::LABEL.canonical).is_some())
            .expect("the label row");
        let series = zerocode_core::jev::promote::on_the_newest_version(&RECALL, &values);
        assert!(series.marks.contains(&named), "the label graded no asking: {named}");
        let judged = zerocode_core::jev::promote::judge_seat(&RECALL, &values).expect("recall rises");
        assert_eq!(
            (judged.agreement.compared, judged.agreement.agreed),
            (1, 1),
            "the judge compared the turn's label once: {named}"
        );
        // The asking it grades is the one its reading settled on: of the two
        // askings of these words over these notes, the later — and neither is
        // the showing.
        let asked: Vec<u64> =
            readings.iter().filter(|row| (row.query, row.notes) == (label.query, label.notes)).map(|row| row.at).collect();
        let [before, graded] = asked.as_slice() else {
            panic!("the same words over the same notes, asked twice: {readings:?}");
        };
        assert!(before < graded, "{asked:?}");
        assert_eq!(label.request_at, Some(*graded), "the label names its reading's asking: {named}");
        assert!(
            label.shown_at.is_some_and(|shown| Some(shown) != label.request_at),
            "the turn was first shown notes at another time than the graded reading was asked: {named}"
        );
    }

    /// Five label rows say readers were shown the hub and none opened it:
    /// the retriever a session is built with reads them, and the graph no
    /// longer brings the hub in on the seed's words — the demand seam of
    /// `runtime::memory::recall` (93db31bf), fed by this seat's rows.
    #[test]
    fn readers_who_left_a_hub_unopened_five_times_change_what_the_next_recall_reads() {
        let mock = Mock::silent();
        let order = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let vault = vault_with_a_hub(cwd);
            let _vault = VaultEnv::point_at(&vault);
            let ledger = rerank_shadow_path(cwd);
            for at in 0..u64::from(runtime::memory::recall::UNADDRESSED_AFTER_RECALLS) {
                append_shadow_row(&ledger, &hub_unopened(at, &vault), SHADOW_LEDGER_MAX_BYTES).expect("a label");
            }
            let retriever = session_retriever(cwd);
            slugs(&retriever.recall("vellichor", 5))
        });
        assert_eq!(order, ["wiki/seed", "wiki/a-quiet"], "five readers left the hub unopened and the graph still brought it in");
    }

    /// A label row saying the turn read the seed and left the hub unopened —
    /// the row this seat writes, as another turn of this project wrote it.
    fn hub_unopened(at: u64, vault: &Path) -> RerankLabelRow {
        let note = |slug: &str, rank: usize, read: bool| ShownNote { slug: slug.to_string(), rank, read, cited: false };
        RerankLabelRow {
            at: 1_000 + at,
            label: format!("{at}:{at}"),
            request_at: None,
            query: at,
            notes: at,
            applied: false,
            agreed: None,
            rank: None,
            not_compared: None,
            baseline_agreed: None,
            shown_at: Some(900 + at),
            vault: Some(super::super::probe_exec::task_fingerprint("", &vault.to_string_lossy())),
            unfinished: false,
            shown: vec![note("wiki/seed", 0, true), note("wiki/z-hub", 1, false)],
        }
    }

    /// The retriever a session at `cwd` recalls with: the production loader,
    /// with this seat seated as its demand — what `runtime_builder` builds.
    fn session_retriever(cwd: &Path) -> Arc<dyn runtime::MemoryRetriever + Send + Sync> {
        runtime::load_memory_retriever(cwd, None, Some(Arc::new(RerankShadow::at(cwd)))).expect("a vault to recall from")
    }

    /// A turn recalls once per request and reads the same section each time:
    /// its label counts each note once, at the place it first held, and a
    /// note a later request's section adds is counted from that section.
    #[test]
    fn a_turns_requests_show_one_section_and_each_note_is_counted_once() {
        let mock = Mock::serving(200, "{\"not\": \"a reply\"}".to_string());
        let rows = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let first = settle(cwd, "which note answers this (shown, twice)", three().to_vec());
            assert_eq!(slugs(&first), ["wiki/a", "wiki/b", "wiki/c"]);
            // The first request was answered: the runtime says so at the
            // second request's boundary, before its recall.
            heard(cwd, TEST_ATTEMPT, told(&[], true, false));
            let mut later = three().to_vec();
            later.remove(0);
            later.push(hit("wiki/d", "arrived on the second request"));
            let second = settle(cwd, "which note answers this (shown, twice)", later);
            assert_eq!(slugs(&second), ["wiki/b", "wiki/c", "wiki/d"]);
            assert!(note_recall_read(cwd, &turn(vec![read_of(&second[2].entry.path)])));
            labels(cwd)
        });
        assert_eq!(rows.len(), 1, "one turn, one row: {rows:?}");
        let named: Vec<(&str, usize, bool)> =
            rows[0].shown.iter().map(|note| (note.slug.as_str(), note.rank, note.read)).collect();
        assert_eq!(named, [("wiki/a", 0, false), ("wiki/b", 1, false), ("wiki/c", 2, false), ("wiki/d", 2, true)]);
    }

    /// The section names the first `MAX_RECALLED_ENTRIES` notes and no more,
    /// and the apply road's bottom level is out of it: a note shown to nobody
    /// is on no label.
    #[test]
    fn a_note_the_section_did_not_name_was_shown_to_nobody() {
        let mock = Mock::serving(200, "{\"not\": \"a reply\"}".to_string());
        let many: Vec<MemoryHit> = (0..7).map(|n| hit(&format!("wiki/n{n}"), "one of many")).collect();
        let shown = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (shown, seven)", many.clone());
            assert_eq!(read.len(), 7, "the seat hands back what it was given");
            assert!(note_recall_read(cwd, &turn(vec![])));
            labels(cwd).pop().expect("a label row").shown
        });
        assert_eq!(
            shown.iter().map(|note| note.slug.as_str()).collect::<Vec<_>>(),
            ["wiki/n0", "wiki/n1", "wiki/n2", "wiki/n3", "wiki/n4"],
            "the section's {MAX_RECALLED_ENTRIES} and no more"
        );

        let mock = Mock::serving(200, reply_for(&[1, 3, 0]));
        let shown = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (shown, dropped)", three().to_vec());
            assert_eq!(slugs(&read), ["wiki/b", "wiki/a"], "the bottom level is out of the turn");
            assert!(note_recall_read(cwd, &turn(vec![])));
            labels(cwd).pop().expect("a label row").shown
        });
        assert_eq!(
            shown.iter().map(|note| (note.slug.as_str(), note.rank)).collect::<Vec<_>>(),
            [("wiki/b", 0), ("wiki/a", 1)],
            "a dropped note was shown to nobody"
        );
    }

    /// A row from before t-6264 names no notes, and a demand folded from it
    /// says nothing about them: an absent column is not "nobody opened it".
    #[test]
    fn an_older_label_names_no_notes_and_folds_to_nothing() {
        let before: RerankLabelRow = serde_json::from_value(serde_json::json!({
            "at": 1, "label": "1:1", "query": 1, "notes": 1, "applied": false, "agreed": false, "rank": 1,
        }))
        .expect("a row from before");
        assert!(before.shown.is_empty() && before.shown_at.is_none() && before.vault.is_none());
        assert!(recall_demand_from([&before], None).is_empty());
    }

    /// The fold: a page is shown once per row it appears on — twice on one
    /// row is once — and opened once when either observation says so; a row
    /// written for another vault is not this vault's evidence, and a slugless
    /// note is nobody's.
    #[test]
    fn the_fold_counts_a_showing_per_turn_and_an_opening_once() {
        let note = |slug: &str, read: bool, cited: bool| ShownNote { slug: slug.to_string(), rank: 0, read, cited };
        let row = |vault: Option<u64>, shown: Vec<ShownNote>| RerankLabelRow {
            at: 1,
            label: "1:1".to_string(),
            request_at: None,
            query: 1,
            notes: 1,
            applied: false,
            agreed: None,
            rank: None,
            not_compared: None,
            baseline_agreed: None,
            shown_at: Some(1),
            vault,
            unfinished: false,
            shown,
        };
        let rows = [
            row(Some(7), vec![note("wiki/x", true, true), note("wiki/x", false, false), note("wiki/y", false, false), note("", true, false)]),
            row(Some(7), vec![note("wiki/x", false, true)]),
            row(Some(7), vec![note("wiki/y", false, false)]),
            row(Some(8), vec![note("wiki/x", false, false), note("wiki/z", false, false)]),
            row(None, vec![note("gotcha-store", false, false)]),
        ];
        let demand = recall_demand_from(&rows, Some(7));
        assert_eq!(demand.shown("wiki/x"), Some((2, 2)), "shown twice, opened both times");
        assert_eq!(demand.shown("wiki/y"), Some((2, 0)));
        assert_eq!(demand.shown("wiki/z"), None, "another vault's page");
        assert_eq!(demand.shown(""), None, "a slugless note is nobody's");
        assert_eq!(demand.shown("gotcha-store"), None, "a row written under no vault is not this vault's");
        assert_eq!(demand.len(), 2);
        assert_eq!(recall_demand_from(&rows, None).shown("gotcha-store"), Some((1, 0)));
    }

    /// The record-only roads rank on nothing: under `off` and `shadow` the
    /// same five rows leave recall's order and its rendered section byte for
    /// byte as a retriever with no seat produces them — the hub still comes
    /// in. That is what a record-only mode promises, proved at the seam a
    /// session recalls through.
    #[test]
    fn under_a_record_only_mode_the_rows_change_nothing_the_turn_reads() {
        for mode in [zerocode_core::jev::JevMode::Off, zerocode_core::jev::JevMode::Shadow] {
            let mock = Mock::silent();
            machine(mode.key(), &mock.base_url, |cwd| {
                let vault = vault_with_a_hub(cwd);
                let _vault = VaultEnv::point_at(&vault);
                let ledger = rerank_shadow_path(cwd);
                for at in 0..u64::from(runtime::memory::recall::UNADDRESSED_AFTER_RECALLS) {
                    append_shadow_row(&ledger, &hub_unopened(at, &vault), SHADOW_LEDGER_MAX_BYTES).expect("a label");
                }
                let unseated = runtime::load_memory_retriever(cwd, None, None).expect("a vault");
                let seated = session_retriever(cwd);
                for query in ["vellichor", "hiraeth", "komorebi sonder"] {
                    let (was, now) = (unseated.recall(query, 5), seated.recall(query, 5));
                    assert_eq!(was, now, "{}: {query}", mode.key());
                    assert_eq!(
                        runtime::render_recalled_memory_section(&was),
                        runtime::render_recalled_memory_section(&now),
                        "{}: {query}",
                        mode.key()
                    );
                }
                assert_eq!(
                    slugs(&seated.recall("vellichor", 5)),
                    ["wiki/seed", "wiki/z-hub", "wiki/a-quiet"],
                    "{}: the hub still comes in",
                    mode.key()
                );
            });
        }
    }

    /// The demand is asked per recall: a label the last turn wrote sinks the
    /// hub on this recall; one reader opening it brings it back on the next;
    /// and a ledger cut to its newer half is folded again from its start.
    #[test]
    fn a_label_the_last_turn_wrote_is_read_by_the_next_recall() {
        let mock = Mock::silent();
        machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let vault = vault_with_a_hub(cwd);
            let _vault = VaultEnv::point_at(&vault);
            let ledger = rerank_shadow_path(cwd);
            let retriever = session_retriever(cwd);
            let order = || slugs(&retriever.recall("vellichor", 5));
            let (hub_in, hub_out) = (["wiki/seed", "wiki/z-hub", "wiki/a-quiet"].as_slice(), ["wiki/seed", "wiki/a-quiet"].as_slice());
            assert_eq!(order(), hub_in, "no reader has answered yet");
            let wanted = u64::from(runtime::memory::recall::UNADDRESSED_AFTER_RECALLS);
            for at in 0..wanted - 1 {
                append_shadow_row(&ledger, &hub_unopened(at, &vault), SHADOW_LEDGER_MAX_BYTES).expect("a label");
            }
            assert_eq!(order(), hub_in, "inside its survival window");
            append_shadow_row(&ledger, &hub_unopened(wanted, &vault), SHADOW_LEDGER_MAX_BYTES).expect("a label");
            assert_eq!(order(), hub_out, "the fifth unopened showing, folded by this recall");
            let mut opened = hub_unopened(wanted + 1, &vault);
            opened.shown[1].cited = true;
            append_shadow_row(&ledger, &opened, SHADOW_LEDGER_MAX_BYTES).expect("a label");
            assert_eq!(order(), hub_in, "one reader citing it is an answer");
            // Cut to its newer half — one unopened showing left.
            let cut = serde_json::to_string(&hub_unopened(50, &vault)).expect("a row");
            std::fs::write(&ledger, format!("{cut}\n")).expect("a cut ledger");
            assert_eq!(order(), hub_in, "folded again from the start of the cut ledger");
            for at in 1..wanted {
                append_shadow_row(&ledger, &hub_unopened(50 + at, &vault), SHADOW_LEDGER_MAX_BYTES).expect("a label");
            }
            assert_eq!(order(), hub_out, "five unopened showings since the cut");
            // Replaced by a ledger that has already grown past the old fold:
            // its first row is another, and nothing of the old fold stands.
            let folded = std::fs::metadata(&ledger).expect("a ledger").len();
            let mut replaced = String::new();
            let mut at = 100;
            while u64::try_from(replaced.len()).unwrap_or(u64::MAX) <= folded {
                let mut opened = hub_unopened(at, &vault);
                opened.shown[1].read = true;
                replaced.push_str(&serde_json::to_string(&opened).expect("a row"));
                replaced.push('\n');
                at += 1;
            }
            std::fs::write(&ledger, replaced).expect("a replaced ledger");
            assert_eq!(order(), hub_in, "a replaced ledger is folded from its own start");
        });
    }

    /// `auto` ranks on its rows only once its own evidence raised it: before
    /// the rise the same rows are recorded and nothing more; after it they
    /// rank, as `on` would.
    #[test]
    fn auto_ranks_on_its_rows_only_once_its_evidence_raised_it() {
        let mock = Mock::silent();
        machine(zerocode_core::jev::JevMode::Auto.key(), &mock.base_url, |cwd| {
            let vault = vault_with_a_hub(cwd);
            let _vault = VaultEnv::point_at(&vault);
            let ledger = rerank_shadow_path(cwd);
            for at in 0..u64::from(runtime::memory::recall::UNADDRESSED_AFTER_RECALLS) {
                append_shadow_row(&ledger, &hub_unopened(at, &vault), SHADOW_LEDGER_MAX_BYTES).expect("a label");
            }
            assert!(demand_for(cwd).is_none(), "an unraised auto records only");
            let retriever = session_retriever(cwd);
            assert_eq!(slugs(&retriever.recall("vellichor", 5)), ["wiki/seed", "wiki/z-hub", "wiki/a-quiet"]);
            let rose = zerocode_core::jev::promote::transition_row(
                &RECALL,
                9_000,
                zerocode_core::jev::promote::Verdict::Rise,
                &zerocode_core::jev::summary::Tally::default(),
            )
            .expect("a rise");
            append_shadow_row(&ledger, &rose, SHADOW_LEDGER_MAX_BYTES).expect("the judge's row");
            assert!(
                demand_for(cwd).is_some_and(|demand| demand.unaddressed("wiki/z-hub")),
                "a raised auto ranks on its rows"
            );
            assert_eq!(slugs(&retriever.recall("vellichor", 5)), ["wiki/seed", "wiki/a-quiet"]);
        });
    }

    /* ---- what reached the model, what the turn did, which ledger (t-6264 r2) */

    /// Notes a recall handed to a request that never left — the context
    /// budget refused it and the runtime took the reminder back — were shown
    /// to nobody: five such turns leave no unopened showing, and the graph
    /// still brings the hub in on the seed's words (astra R1a).
    #[test]
    fn an_undispatched_recall_adds_no_unopened_showing() {
        let mock = Mock::serving(200, "{\"not\": \"a reply\"}".to_string());
        let hub = || vec![hit("wiki/seed", "vellichor"), hit("wiki/z-hub", "hiraeth")];
        let (order, rows) = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let vault = vault_with_a_hub(cwd);
            let _vault = VaultEnv::point_at(&vault);
            for _ in 0..runtime::memory::recall::UNADDRESSED_AFTER_RECALLS {
                let _read = settle(cwd, "vellichor", hub());
                // The request never left: the runtime took the reminder back
                // and the turn ended on the budget error, telling the seat
                // nothing more (`runtime::TurnProgress`).
                assert!(!super::note_recall_read(cwd, TEST_ATTEMPT, false), "a turn shown nothing wrote a row");
            }
            // A turn whose first request was answered keeps what that request
            // showed, though its second — which would have added a page —
            // never left.
            let first = settle(cwd, "vellichor", hub());
            heard(cwd, TEST_ATTEMPT, told(&turn(vec![read_of(&first[0].entry.path)]), true, false));
            let mut more = hub();
            more.push(hit("wiki/a-quiet", "sonder"));
            let _second = settle(cwd, "vellichor", more);
            assert!(super::note_recall_read(cwd, TEST_ATTEMPT, false));
            (slugs(&session_retriever(cwd).recall("vellichor", 5)), labels(cwd))
        });
        assert_eq!(
            order,
            ["wiki/seed", "wiki/z-hub", "wiki/a-quiet"],
            "notes that never reached the model were counted as left unopened"
        );
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(
            row.shown.iter().map(|note| (note.slug.as_str(), note.read)).collect::<Vec<_>>(),
            [("wiki/seed", true), ("wiki/z-hub", false)],
            "the answered request's showing, and nothing of the one that never left"
        );
        assert!(row.unfinished, "a turn that failed is not a whole record");
        let demand = recall_demand_from(&rows, row.vault);
        assert_eq!(demand.shown("wiki/seed"), Some((1, 1)), "the read it made counts");
        assert_eq!(demand.shown("wiki/z-hub"), None, "what a failed turn left unread is unknown, not unopened");
    }

    /// A turn that read a note did read it, whatever the transcript holds
    /// when its labels are written: a compaction — mid-turn, or the one after
    /// the turn's last answer — summarises the read away, and the label must
    /// not call the note unopened (astra R1b).
    #[test]
    fn a_compacted_read_is_not_relabelled_unopened() {
        let mock = Mock::serving(200, "{\"not\": \"a reply\"}".to_string());
        let label = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let read = settle(cwd, "which note answers this (compacted)", three().to_vec());
            // What the turn did: it read the first note — handed to the seat at
            // the request's boundary, before the compaction there — then
            // answered, and ended.
            let did = turn(vec![read_of(&read[0].entry.path)]);
            heard(cwd, TEST_ATTEMPT, told(&did, true, false));
            // What the transcript held when the labels were written — the
            // compaction's summary and the answer it kept — is no longer what
            // the label reads.
            let kept = [
                ConversationMessage::user_text("<summary of the work so far>"),
                ConversationMessage::assistant(vec![said("done")]),
            ];
            heard(cwd, TEST_ATTEMPT, told(&kept[1..], false, true));
            assert!(super::note_recall_read(cwd, TEST_ATTEMPT, false));
            labels(cwd).pop().expect("a label row")
        });
        assert!(label.shown[0].read, "a read the compaction took was relabelled unopened: {label:?}");
        assert!(!label.unfinished, "{label:?}");
    }

    /// The assistant's own citation is a link in its own words: a line it
    /// quotes from someone else (`> … [[wiki/a]]`) and a fenced block it
    /// shows are not its citation of those pages (astra R1c).
    #[test]
    fn a_quoted_wikilink_is_not_the_assistants_own_citation() {
        let mock = Mock::serving(200, "{\"not\": \"a reply\"}".to_string());
        let label = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let _read = settle(cwd, "which note answers this (quoted)", three().to_vec());
            let reply = "The page puts it this way:\n> the old rule lives in [[wiki/a]]\n\n```text\n[[wiki/b]] is how a link is written\n```\n\nso I follow [[wiki/c]].";
            assert!(note_recall_read(cwd, &turn(vec![said(reply)])));
            labels(cwd).pop().expect("a label row")
        });
        let cited: Vec<(&str, bool)> = label.shown.iter().map(|note| (note.slug.as_str(), note.cited)).collect();
        assert_eq!(
            cited,
            [("wiki/a", false), ("wiki/b", false), ("wiki/c", true)],
            "a quoted or fenced link was read as the assistant's own citation"
        );
    }

    /// A quote runs past the lines that open with `>`: a line that carries
    /// the quoted paragraph on without one — a lazy continuation, in
    /// `CommonMark` and in the renderer that draws the answer — is still the
    /// quoted words, and so is one that carries on a quote inside a quote,
    /// and a fence the quote holds. The quote ends with its paragraph: after
    /// a blank line the words are the assistant's own again (astra R1c, r3).
    #[test]
    fn a_quote_runs_on_through_its_lazy_lines_and_ends_at_a_blank_line() {
        let mock = Mock::serving(200, "{\"not\": \"a reply\"}".to_string());
        let five = ["wiki/a", "wiki/b", "wiki/c", "wiki/d", "wiki/e"].map(|slug| hit(slug, "one of five"));
        let label = machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let _read = settle(cwd, "which note answers this (lazy quote)", five.to_vec());
            let reply = "The page puts it this way:\n\
                         > the old rule lives in [[wiki/a]], and it\n\
                         was moved by [[wiki/b]] later.\n\
                         \n\
                         > > the note before that one\n\
                         > > said\n\
                         [[wiki/c]] was its source,\n\
                         > ```text\n\
                         > [[wiki/d]]\n\
                         > ```\n\
                         \n\
                         so I follow [[wiki/e]].";
            assert!(note_recall_read(cwd, &turn(vec![said(reply)])));
            labels(cwd).pop().expect("a label row")
        });
        let cited: Vec<(&str, bool)> = label.shown.iter().map(|note| (note.slug.as_str(), note.cited)).collect();
        assert_eq!(
            cited,
            [("wiki/a", false), ("wiki/b", false), ("wiki/c", false), ("wiki/d", false), ("wiki/e", true)],
            "a quote's lazy lines were read as the assistant's own citations"
        );
    }

    /// The assistant's own words, block by block, as `CommonMark` reads an
    /// answer: a quote's lazy line quotes a path as much as a link, and an
    /// indented block is code; a line that opens a list item or a heading
    /// is no lazy line — it ends the quoted paragraph — and neither is one
    /// after a blank line, so what they cite is the assistant's own
    /// (astra R1c, r3).
    #[test]
    fn own_citations_read_an_answer_the_way_commonmark_renders_it() {
        let answers: [(&str, &[&str]); 7] = [
            ("> quoted\nwiki/lazy.md carries the quote on", &[]),
            ("> > nested\ncarries it on to [[wiki/nested]]", &[]),
            ("my words\n\n    [[wiki/indented]] is code\n", &[]),
            ("```\n[[wiki/fenced]]\n```\nthen [[wiki/own]]", &["wiki/own"]),
            ("> quoted\n- my pick is [[wiki/item]]", &["wiki/item"]),
            ("> quoted\n# [[wiki/heading]]", &["wiki/heading"]),
            ("> quoted [[wiki/quoted]]\n\nso [[wiki/after]]", &["wiki/after"]),
        ];
        for (answer, cited) in answers {
            assert_eq!(own_citations(answer), cited, "{answer:?}");
        }
    }

    /// A ledger replaced by another of the very same length — a restore, a
    /// copy, a cut grown back to the length it had — is another ledger: the
    /// demand a warm reader folded and the standing it read are not the new
    /// ledger's, and each is read again, agreeing with a reader that never
    /// saw the old one (astra R2).
    #[test]
    fn a_same_length_ledger_replacement_invalidates_demand_and_standing() {
        let mock = Mock::silent();
        machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let vault = vault_with_a_hub(cwd);
            let _vault = VaultEnv::point_at(&vault);
            let ledger = rerank_shadow_path(cwd);
            std::fs::create_dir_all(ledger.parent().expect("a ledger dir")).expect("a ledger dir");
            // Turns that left the page each names unopened, one row each from
            // `from` on — every row the same length whatever the page.
            let written = |pages: &[&str], from: u64| -> String {
                pages
                    .iter()
                    .zip(from..)
                    .map(|(page, at)| {
                        let mut row = hub_unopened(at, &vault);
                        row.shown[1].slug = (*page).to_string();
                        serde_json::to_string(&row).expect("a row") + "\n"
                    })
                    .collect()
            };
            let asked = |demand: Option<&RecallDemand>| {
                demand.map_or((false, false), |demand| (demand.unaddressed("wiki/z-hub"), demand.unaddressed("wiki/y-hub")))
            };
            let warm = || asked(demand_for(cwd).as_deref());
            let cold = || asked(Some(&recall_demand_from(&labels(cwd), vault_fingerprint())));
            let retriever = session_retriever(cwd);
            let order = || slugs(&retriever.recall("vellichor", 5));
            let (hub_in, hub_out) = (["wiki/seed", "wiki/z-hub", "wiki/a-quiet"], ["wiki/seed", "wiki/a-quiet"]);
            let hub = written(&["wiki/z-hub"; 5], 1_000);
            std::fs::write(&ledger, &hub).expect("a ledger");
            assert_eq!(order(), hub_out, "five unopened showings sink the hub");
            // The same length and the same first bytes, another page unopened.
            let other = written(&["wiki/y-hub"; 5], 1_000);
            assert_eq!(other.len(), hub.len());
            std::fs::write(&ledger, &other).expect("a replaced ledger");
            assert_eq!(warm(), cold(), "a warm fold kept the replaced ledger's demand");
            assert_eq!(order(), hub_in, "the replacing ledger never left the hub unopened");
            // Cut to its newer rows, and grown back to the very length it had.
            std::fs::write(&ledger, &hub).expect("the hub's ledger again");
            assert_eq!(order(), hub_out);
            let regrown = written(&["wiki/z-hub"; 4], 1_001) + &written(&["wiki/y-hub"], 1_005);
            assert_eq!(regrown.len(), hub.len());
            std::fs::write(&ledger, &regrown).expect("a cut ledger grown back");
            assert_eq!(warm(), cold(), "a warm fold kept a row the cut removed");
            assert_eq!(order(), hub_in, "four unopened showings since the cut");
        });
        // The standing an `auto` seat reads, both ways, against the common
        // reader: `rise` and `fall` are one length.
        let mock = Mock::silent();
        machine(zerocode_core::jev::JevMode::Auto.key(), &mock.base_url, |cwd| {
            use zerocode_core::jev::promote::{FELL, ROSE};
            let ledger = rerank_shadow_path(cwd);
            std::fs::create_dir_all(ledger.parent().expect("a ledger dir")).expect("a ledger dir");
            let stood = |word: &str| format!("{{\"at\":9000,\"transition\":\"{word}\"}}\n");
            for (from, to) in [(ROSE, FELL), (FELL, ROSE)] {
                std::fs::write(&ledger, stood(from)).expect("a ledger");
                assert_eq!(raised_now(cwd), runtime::jev_seat_applies(cwd, &RECALL), "{from}");
                std::fs::write(&ledger, stood(to)).expect("a replaced ledger");
                assert_eq!(
                    raised_now(cwd),
                    runtime::jev_seat_applies(cwd, &RECALL),
                    "{from} → {to}: the standing read off the replaced ledger"
                );
            }
        });
    }

    /// One label row's line, as a turn wrote it: shown the seed, which it
    /// read, and `page`, `opened` or not — the same length for any page
    /// whose name is as long.
    fn showing_line(at: u64, page: &str, opened: bool, vault: &Path) -> String {
        let mut row = hub_unopened(at, vault);
        row.shown[1].slug = page.to_string();
        row.shown[1].read = opened;
        serde_json::to_string(&row).expect("a row") + "\n"
    }

    /// This project's demand as the seat folds it, warm: the fold this
    /// process keeps, caught up now — before any setting is read.
    fn folded(cwd: &Path) -> RecallDemand {
        let ledger = rerank_shadow_path(cwd);
        let mut book = demand_book().lock().expect("the fold's book");
        RecallDemand::clone(&book.entry(ledger.clone()).or_default().catch_up(&ledger, vault_fingerprint()))
    }

    /// The same ledger's demand read cold, by a reader that never saw it.
    fn folded_cold(cwd: &Path) -> RecallDemand {
        recall_demand_from(&labels(cwd), vault_fingerprint())
    }

    /// Append `text` to `ledger` as a writer appends a row.
    fn appended(ledger: &Path, text: &str) {
        let mut file = std::fs::OpenOptions::new().append(true).open(ledger).expect("a ledger to append to");
        std::io::Write::write_all(&mut file, text.as_bytes()).expect("appended");
    }

    /// Rewrite `ledger`, which holds `held`, in place — the same file,
    /// truncated and written again — to `text`, the very length, keeping
    /// its first row's worth and the row's worth where it ended; then grow
    /// it by `grown`. Every mark a fold that trusted its birth, its first
    /// bytes and the row where it stopped would read is the ledger's before
    /// the rewrite (astra R2, r3).
    fn rewrite_in_place_then_grow(ledger: &Path, held: &str, text: &str, grown: &str) {
        let row = usize::try_from(TAIL_ROW_BYTES).expect("a row's worth");
        assert_eq!(text.len(), held.len(), "a rewrite of the very length");
        assert_eq!(text[..row], held[..row], "the first row's worth kept");
        assert_eq!(text[text.len() - row..], held[held.len() - row..], "the row's worth where the fold stopped kept");
        #[cfg(unix)]
        let file = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(ledger).expect("a ledger"));
        std::fs::write(ledger, text).expect("rewritten in place");
        appended(ledger, grown);
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(ledger).expect("a ledger")),
            file,
            "the same file, rewritten"
        );
    }

    /// A ledger rewritten in place — the same file, its first row and the
    /// rows up to where the fold stopped kept byte for byte, five rows
    /// between them now about another page — and then grown is not a ledger
    /// that only grew: a fold that took it for one went on ranking on the
    /// rewritten rows' old answers, where a reader that never saw the old
    /// ledger ranks on the new. What the fold read is checked, window by
    /// window, against the ledger now there, and a window that no longer
    /// holds starts the fold again from the start; what is appended after is
    /// folded on from there (astra R2, r3).
    #[test]
    fn a_ledger_rewritten_in_place_then_grown_is_folded_again() {
        let mock = Mock::silent();
        machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let vault = vault_with_a_hub(cwd);
            let _vault = VaultEnv::point_at(&vault);
            let ledger = rerank_shadow_path(cwd);
            std::fs::create_dir_all(ledger.parent().expect("a ledger dir")).expect("a ledger dir");
            let line = |at: u64, page: &str, opened: bool| showing_line(at, page, opened, &vault);
            // A page every reader opened, a row's worth and more on either
            // side of five turns that left one page unopened.
            let opened = |from: u64| (from..from + 12).map(|at| line(at, "wiki/a-quiet", true)).collect::<String>();
            let unopened = |page: &str| (50..55).map(|at| line(at, page, false)).collect::<String>();
            let retriever = session_retriever(cwd);
            let order = || slugs(&retriever.recall("vellichor", 5));
            let (hub_in, hub_out) = (["wiki/seed", "wiki/z-hub", "wiki/a-quiet"], ["wiki/seed", "wiki/a-quiet"]);
            let held = opened(0) + &unopened("wiki/z-hub") + &opened(100);
            std::fs::write(&ledger, &held).expect("a ledger");
            assert_eq!(order(), hub_out, "five unopened showings sink the hub");
            assert_eq!(folded(cwd), folded_cold(cwd));
            // The five rows now about a page the vault does not hold, and a
            // row appended past where the fold stopped.
            let rewritten = opened(0) + &unopened("wiki/y-hub") + &opened(100);
            rewrite_in_place_then_grow(&ledger, &held, &rewritten, &line(200, "wiki/a-quiet", true));
            assert_eq!(folded(cwd), folded_cold(cwd), "a warm fold kept the rewritten rows' old answers");
            assert_eq!(order(), hub_in, "no reader left the hub unopened in the ledger now there");
            // And what is appended is folded on: four turns more leave the
            // hub in, the fifth sinks it.
            for at in 300..305 {
                appended(&ledger, &line(at, "wiki/z-hub", false));
                assert_eq!(folded(cwd), folded_cold(cwd), "row {at} appended");
                assert_eq!(order(), if at < 304 { hub_in.as_slice() } else { hub_out.as_slice() }, "row {at} appended");
            }
        });
    }

    /// The standing an `auto` seat reads and the demand it would rank on are
    /// read off one ledger: rewritten in place and grown, both answer the
    /// ledger now there — whichever way the rewrite turned its last
    /// transition, and whichever page it left unopened (astra R2, r3).
    #[test]
    fn after_an_in_place_rewrite_the_standing_and_the_demand_read_one_ledger() {
        use zerocode_core::jev::promote::{FELL, ROSE};
        let mock = Mock::silent();
        machine(zerocode_core::jev::JevMode::Auto.key(), &mock.base_url, |cwd| {
            let vault = vault_with_a_hub(cwd);
            let _vault = VaultEnv::point_at(&vault);
            let ledger = rerank_shadow_path(cwd);
            std::fs::create_dir_all(ledger.parent().expect("a ledger dir")).expect("a ledger dir");
            let line = |at: u64, page: &str, opened: bool| showing_line(at, page, opened, &vault);
            let opened = |from: u64| (from..from + 12).map(|at| line(at, "wiki/a-quiet", true)).collect::<String>();
            // `rise` and `fall` are one length, as the two pages' names are.
            let middle = |word: &str, page: &str| {
                format!("{{\"at\":9000,\"transition\":\"{word}\"}}\n") + &(50..55).map(|at| line(at, page, false)).collect::<String>()
            };
            let warm = || (raised_now(cwd), folded(cwd));
            let cold = || (runtime::jev_seat_applies(cwd, &RECALL), folded_cold(cwd));
            for (grown, (from, to)) in (200..).zip([(ROSE, FELL), (FELL, ROSE)]) {
                let held = opened(0) + &middle(from, "wiki/z-hub") + &opened(100);
                std::fs::write(&ledger, &held).expect("a ledger");
                assert_eq!(warm(), cold(), "{from}");
                let rewritten = opened(0) + &middle(to, "wiki/y-hub") + &opened(100);
                rewrite_in_place_then_grow(&ledger, &held, &rewritten, &line(grown, "wiki/a-quiet", true));
                assert_eq!(warm(), cold(), "{from} → {to}: the standing or the demand read the ledger the rewrite replaced");
            }
        });
    }

    /// A label that names its own showing (`shown`, `shownAt`), for the
    /// replay's fixtures: one page, shown at `shown_at` on a reading `key`
    /// names, its label written at `at`.
    fn named_showing(key: u64, at: u64, shown_at: u64, opened: bool) -> serde_json::Value {
        serde_json::json!({
            "at": at, "label": format!("{key}:{key}"), "query": key, "notes": key, "applied": false, "shownAt": shown_at,
            "shown": [{"slug": "wiki/x", "rank": 0, "read": opened, "cited": false}],
        })
    }

    /// Recall's order of the two pages the replay's older readings are about.
    const XY: [&str; 2] = ["wiki/x", "wiki/y"];

    /// A reading's row as the ledger held it before labels named their
    /// showings, for the replay's fixtures: one question over pages x and y,
    /// `key` naming both fingerprints, judged into `proposed` and recorded
    /// beside recall's order.
    fn older_reading(at: u64, key: u64, proposed: [&str; 2]) -> serde_json::Value {
        serde_json::json!({
            "at": at, "query": key, "notes": key, "rubric_version": RERANK_RUBRIC_VERSION,
            "outcome": RERANK_OUTCOME_ANSWERED, "candidates": 2, "applied": false,
            "judged": {"recalled": XY, "proposed": proposed, "moved": 0,
                       "top_changed": false, "held_by_graph": [], "readings": [[1.0, 0.9], [0.5, 0.9]]},
        })
    }

    /// A label from before labels named their showings: the reading it
    /// grades, by the fingerprints `key` names, and its `marks`.
    fn older_label(at: u64, key: u64, marks: &serde_json::Value) -> serde_json::Value {
        let mut label = serde_json::json!({"at": at, "label": format!("{key}:{key}"), "query": key, "notes": key, "applied": false});
        if let (Some(label), Some(marks)) = (label.as_object_mut(), marks.as_object()) {
            label.extend(marks.clone());
        }
        label
    }

    /// An older label's marks for a turn that touched the judgment's first
    /// note, or `agreed: false` ones for a turn that did not.
    fn agreed(agreed: bool) -> serde_json::Value {
        serde_json::json!({"agreed": agreed, "rank": 0, "baselineAgreed": agreed})
    }

    /// A showing is ranked on what was known when it was shown. Turn A
    /// showed page x when four readers had left it unopened; turn B's label
    /// made it five before A's own label arrived — so A's showing is not
    /// one the demand would have changed, and B's was not either (astra R3).
    #[test]
    fn a_late_label_cannot_change_an_earlier_showings_demand() {
        let window = runtime::memory::recall::UNADDRESSED_AFTER_RECALLS;
        let mut rows: Vec<serde_json::Value> = (0..u64::from(window) - 1).map(|n| named_showing(7, 100 + n, 50 + n, false)).collect();
        // B: shown at 1,500, labeled at 2,000 — the fifth unopened showing.
        rows.push(named_showing(7, 2_000, 1_500, false));
        // A: shown at 1,000, before B was; labeled at 3,000, after B was.
        rows.push(named_showing(7, 3_000, 1_000, false));
        let replayed = replay_at(&rows, window);
        assert_eq!(replayed.named.exposures, rows.len(), "{replayed:?}");
        assert_eq!(
            replayed.named.changed, 0,
            "a label written after a showing changed what that showing was ranked on: {replayed:?}"
        );
        // The one that follows both is ranked on all five.
        rows.push(named_showing(7, 4_000, 3_500, false));
        assert_eq!(replay_at(&rows, window).named.changed, 1);
    }

    /// Two turns that asked one question over the same notes — two windows
    /// on one project — are two showings. An older label names its reading
    /// only by those two fingerprints, so when two labels answer one run of
    /// requests the replay cannot say which is whose, and counts both apart
    /// rather than joining one and dropping the other; a label that names
    /// its own showing is its own, however alike the readings (astra R3).
    /// And one label on a run of two requests is no more one turn's than
    /// two's: it is ranked only on the assumption that it was, apart from
    /// the confirmed showings (astra R3, r3).
    #[test]
    fn two_turns_with_the_same_reading_are_not_one_exposure() {
        let window = runtime::memory::recall::UNADDRESSED_AFTER_RECALLS;
        let reading = |at: u64| older_reading(at, 7, XY);
        let rows = [reading(1_000), reading(1_100), older_label(2_000, 7, &agreed(true)), older_label(2_100, 7, &agreed(false))];
        let replayed = replay_at(&rows, window);
        assert_eq!(
            (replayed.confirmed.exposures + replayed.conditional.exposures, replayed.ambiguous, replayed.orphan),
            (0, 2, 0),
            "two turns' labels were joined as one showing: {replayed:?}"
        );
        // One label on a run of two requests: one turn's, or two turns' with
        // one label lost — ranked only as the assumption it is.
        let replayed = replay_at(&[reading(1_000), reading(1_100), older_label(2_000, 7, &agreed(true))], window);
        assert_eq!(
            (replayed.confirmed.exposures, replayed.conditional.exposures, replayed.repeated, replayed.ambiguous),
            (0, 1, 1, 0),
            "a run of requests and one label was confirmed as one turn's showing: {replayed:?}"
        );
        let rows = [reading(1_000), reading(1_100), named_showing(7, 2_000, 1_000, true), named_showing(7, 2_100, 1_100, false)];
        assert_eq!(replay_at(&rows, window).named.exposures, 2);
    }

    /// Two turns asked one question over the same notes; one was cancelled
    /// and wrote no label, the other did. An older label names its reading
    /// by two fingerprints only, so which request of the run it answers was
    /// its showing cannot be told — and taken for the run's first, the
    /// showing is ranked before a label its real showing came after, on a
    /// demand it never met. So a run of more than one request is no
    /// confirmed showing: the replay that takes each run for one turn's
    /// ranks it apart, as the assumption it is, and one request with its one
    /// label is confirmed (astra R3, r3).
    #[test]
    fn one_label_left_of_two_turns_is_not_a_confirmed_showing() {
        let window = runtime::memory::recall::UNADDRESSED_AFTER_RECALLS;
        let untouched = serde_json::json!({"notCompared": NO_NOTE_TOUCHED});
        // Four turns left page x unopened, each naming its own showing.
        let mut rows: Vec<serde_json::Value> = (0..u64::from(window) - 1).map(|n| named_showing(1, 10 + n, 5 + n, false)).collect();
        // A asks at 100 and is cancelled — no label; B asks the same at 200.
        rows.extend([older_reading(100, 7, XY), older_reading(200, 7, XY)]);
        // Another turn's label, at 150, leaves x unopened a fifth time.
        rows.push(named_showing(2, 150, 120, false));
        // B's label, at 300, from before labels named their showings.
        rows.push(older_label(300, 7, &untouched));
        // C asks another question once, at 400, and labels it at 500.
        rows.extend([older_reading(400, 8, XY), older_label(500, 8, &untouched)]);
        let replayed = replay_at(&rows, window);
        assert_eq!(
            (replayed.confirmed.exposures, replayed.conditional.exposures, replayed.ambiguous),
            (1, 1, 0),
            "one label on a run of two requests was confirmed as one turn's showing: {replayed:?}"
        );
        // C was shown x after five turns and B had left it unopened: the
        // demand sinks it there.
        assert_eq!(replayed.confirmed.changed, 1, "{replayed:?}");
    }

    /// An older label's marks point into its reading's order — `rank` a
    /// place in the judgment's list, `agreed` its first — so when the
    /// requests of the run it answers were judged in two orders it does not
    /// say which page its turn opened, and nothing it says is folded: the
    /// run's first order named a page opened that the turn may never have
    /// read (astra R3, r3).
    #[test]
    fn a_label_on_requests_judged_in_two_orders_names_no_page() {
        let window = runtime::memory::recall::UNADDRESSED_AFTER_RECALLS;
        let first_touched = serde_json::json!({"agreed": true, "rank": 0});
        let rows = [older_reading(100, 7, XY), older_reading(200, 7, ["wiki/y", "wiki/x"]), older_label(300, 7, &first_touched)];
        let replayed = replay_at(&rows, window);
        assert_eq!(
            (replayed.undetermined, replayed.observed),
            (1, 0),
            "a page was folded as opened off one of two orders the label may have graded: {replayed:?}"
        );
    }

    /// A label closes its run, but not every turn of it: A and B asked one
    /// question at 100 and 200, A's label came at 300 while B still ran, C
    /// asked it once more at 400 and was cancelled, and B's label came at
    /// 500. The rows cannot say whether 500 is B's or C's, so it is no
    /// confirmed showing — and taken for C's at 400, it ranks page x after
    /// five unopened turns when B's showing met four. Nor is a request two
    /// runs on, and a run every label answered carries nothing on: its next
    /// run of one request is confirmed (astra R3, r4).
    #[test]
    fn a_label_a_closed_run_still_owes_is_no_confirmed_showing() {
        let window = runtime::memory::recall::UNADDRESSED_AFTER_RECALLS;
        let untouched = serde_json::json!({"notCompared": NO_NOTE_TOUCHED});
        // Four turns left page x unopened, each naming its own showing.
        let named: Vec<serde_json::Value> = (0..u64::from(window) - 1).map(|n| named_showing(1, 10 + n, 5 + n, false)).collect();
        let reading = |at: u64| older_reading(at, 7, XY);
        let label = |at: u64| older_label(at, 7, &untouched);
        let crossed = [reading(100), reading(200), label(300), reading(400), label(500)];
        let rows: Vec<_> = named.iter().cloned().chain(crossed.iter().cloned()).collect();
        let replayed = replay_at(&rows, window);
        assert_eq!(replayed.named.exposures, named.len(), "{replayed:?}");
        assert_eq!(
            (replayed.confirmed.exposures, replayed.confirmed.changed, replayed.conditional.exposures),
            (0, 0, 2),
            "a label a run of two requests may still owe was confirmed on the next run's request: {replayed:?}"
        );
        // B's label two runs on: C's came at 500, D asked at 600, B's at 700.
        let rows: Vec<_> = named.iter().cloned().chain(crossed.iter().cloned()).chain([reading(600), label(700)]).collect();
        let replayed = replay_at(&rows, window);
        assert_eq!(
            (replayed.confirmed.exposures, replayed.conditional.exposures),
            (0, 3),
            "a request a closed run may still owe was forgotten after one more run: {replayed:?}"
        );
        // Two labels on the run of two requests: nothing is owed on, and C's
        // one request with its one label is C's showing.
        let settled = [reading(100), reading(200), label(300), label(310), reading(400), label(500)];
        let replayed = replay_at(&settled, window);
        assert_eq!(
            (replayed.confirmed.exposures, replayed.ambiguous),
            (1, 2),
            "a run every label answered still held back the next run's showing: {replayed:?}"
        );
    }

    /// The same crossing, with the readings apart: the run of two requests
    /// judged x before y, the later one y before x — and the other way
    /// round. The late label's mark (`rank` 0, the judgment's first opened)
    /// names x if it is B's and y if it is C's, so it names no page at all,
    /// and nothing it says is folded — whichever turn's label came first,
    /// and whichever order came first. What the rows hold is only A's or
    /// B's label at 300, which names one page either way (astra R3, r4).
    #[test]
    fn a_late_label_folds_only_what_every_turn_it_may_be_would_say() {
        let window = runtime::memory::recall::UNADDRESSED_AFTER_RECALLS;
        let first_touched = serde_json::json!({"agreed": true, "rank": 0});
        let yx = ["wiki/y", "wiki/x"];
        for (owed, later) in [(XY, yx), (yx, XY)] {
            let rows = [
                older_reading(100, 7, owed),
                older_reading(200, 7, owed),
                older_label(300, 7, &first_touched),
                older_reading(400, 7, later),
                older_label(500, 7, &first_touched),
            ];
            let replayed = replay_at(&rows, window);
            assert_eq!(
                (replayed.confirmed.exposures, replayed.undetermined, replayed.observed),
                (0, 1, 1),
                "{owed:?} then {later:?}: the late label was read off the later request's order alone: {replayed:?}"
            );
        }
    }

    /// What the demand costs a recall on this machine, printed: the first
    /// fold of a ledger the size of this machine's
    /// (`ZO_RERANK_REPLAY_LEDGER`, copied; else 1,300 synthetic rows), a
    /// recall whose ledger did not change — one look at it, beside the bare
    /// `stat` a length-only cache paid — the fold of one appended row, with
    /// the settings and without, and the check of every window the fold
    /// read that a changed look pays (t-6264 r3), the settings read the road
    /// costs every recall, and the retriever's recall seated against
    /// unseated.
    ///
    /// ```text
    /// ZO_RERANK_REPLAY_LEDGER=~/.zo/projects/<slug>/state/smart-router/rerank-shadow.jsonl \
    ///   cargo test -p tools --release --lib -- measure_what_the_demand_costs_a_recall --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "measures this machine; run deliberately with --nocapture"]
    fn measure_what_the_demand_costs_a_recall() {
        use std::time::Instant;
        fn median(mut samples: Vec<Duration>) -> Duration {
            samples.sort();
            samples[samples.len() / 2]
        }
        fn timed<T>(times: usize, mut body: impl FnMut() -> T) -> Duration {
            median((0..times).map(|_| {
                let began = Instant::now();
                let _ = body();
                began.elapsed()
            }).collect())
        }
        let mock = Mock::silent();
        machine(zerocode_core::jev::JevMode::On.key(), &mock.base_url, |cwd| {
            let vault = vault_with_a_hub(cwd);
            let _vault = VaultEnv::point_at(&vault);
            let ledger = rerank_shadow_path(cwd);
            std::fs::create_dir_all(ledger.parent().expect("a ledger dir")).expect("a ledger dir");
            let source = if let Some(real) = std::env::var_os("ZO_RERANK_REPLAY_LEDGER") {
                std::fs::copy(&real, &ledger).expect("a copy of the real ledger");
                "this machine's ledger".to_string()
            } else {
                    for at in 0..900u64 {
                        let row = serde_json::json!({
                            "at": 1_000 + at, "query": at, "notes": at, "rubric_version": RERANK_RUBRIC_VERSION,
                            "outcome": RERANK_OUTCOME_ANSWERED, "candidates": 3, "elapsed_ms": 300, "retries": 0,
                            "requests": 1, "redactedLines": 0, "applied": false,
                            "judged": {"recalled": ["wiki/seed", "wiki/z-hub", "wiki/a-quiet"], "proposed": ["wiki/seed", "wiki/z-hub", "wiki/a-quiet"], "moved": 0, "top_changed": false, "held_by_graph": [], "readings": [[1.0, 0.9], [0.3, 0.9], [0.3, 0.9]]},
                        });
                        append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES).expect("a reading");
                    }
                    for at in 0..400u64 {
                        // Turns that read the seed and were shown nothing else.
                        let mut row = hub_unopened(at, &vault);
                        row.shown.truncate(1);
                        append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES).expect("a label");
                    }
                    "1,300 synthetic rows".to_string()
            };
            let bytes = std::fs::metadata(&ledger).map(|meta| meta.len()).unwrap_or(0);
            let rows = std::fs::read_to_string(&ledger).map(|text| text.lines().count()).unwrap_or(0);
            // As the ledger stands: nothing it says sinks a page yet.
            let first = timed(1, || demand_for(cwd));
            let quiet = timed(200, || demand_for(cwd));
            // Then five turns that left the hub unopened: the demand now ranks,
            // and the road is read to say whether it may.
            for at in 0..u64::from(runtime::memory::recall::UNADDRESSED_AFTER_RECALLS) {
                append_shadow_row(&ledger, &hub_unopened(10_000 + at, &vault), SHADOW_LEDGER_MAX_BYTES).expect("a label");
            }
            let demand = demand_for(cwd).expect("on ranks on a hub left unopened");
            let unchanged = timed(200, || demand_for(cwd));
            let appended = timed(50, || {
                append_shadow_row(&ledger, &hub_unopened(9_000, &vault), SHADOW_LEDGER_MAX_BYTES).expect("a label");
                demand_for(cwd)
            });
            // The fold alone on a changed look, before any settings: one row
            // appended — every window the fold read, read again and checked,
            // and the row parsed — and the check by itself.
            let fold_one_row = timed(50, || {
                append_shadow_row(&ledger, &hub_unopened(9_000, &vault), SHADOW_LEDGER_MAX_BYTES).expect("a label");
                folded(cwd)
            });
            let (windows, check) = {
                let book = demand_book().lock().expect("the fold's book");
                let fold = book.get(&ledger).expect("the ledger, folded");
                let mut file = std::fs::File::open(&ledger).expect("the ledger");
                (fold.windows.len(), timed(50, || fold.still_held(&mut file)))
            };
            let mode = timed(200, || rerank_shadow_mode_from(&runtime::ConfigLoader::default_for(cwd)));
            // Under `auto` the road reads the seat's standing: the whole
            // ledger once per state of it, shared by the demand and the road.
            let stand_whole = timed(20, || runtime::jev_seat_applies(cwd, &RECALL));
            let _ = raised_now(cwd);
            let stand_memo = timed(200, || raised_now(cwd));
            let look = timed(200, || LedgerLook::of(&ledger));
            let stat = timed(200, || std::fs::metadata(&ledger).map(|meta| meta.len()));
            let unseated = runtime::load_memory_retriever(cwd, None, None).expect("a vault");
            let seated = session_retriever(cwd);
            let recall_unseated = timed(300, || unseated.recall("vellichor", 5));
            let recall_seated = timed(300, || seated.recall("vellichor", 5));
            println!("\n  ledger: {source} — {rows} rows, {bytes} bytes; demand names {} pages", demand.len());
            println!("  first fold (whole ledger)        {first:?}");
            println!("  demand_for, nothing sunk         {quiet:?}  (one look; no settings read)");
            println!("  one look at the ledger           {look:?}  (open, its metadata, a row's worth at each end)");
            println!("  a bare stat, for comparison      {stat:?}  (what a length-only cache paid)");
            println!("  demand_for, a page sunk          {unchanged:?}  (one look and the settings)");
            println!("  demand_for after one row         {appended:?}  (one row folded, and the settings)");
            println!("  fold after one row, no settings  {fold_one_row:?}  (every window read again and checked, one row parsed)");
            println!("  the windows checked, alone       {check:?}  ({windows} windows of up to {FOLD_WINDOW_BYTES} bytes)");
            println!("  settings read (the mode)         {mode:?}");
            println!("  stand, whole ledger read         {stand_whole:?}  (once per recall under auto, before and after)");
            println!("  stand, second ask same recall    {stand_memo:?}  (the road after the demand: one look)");
            println!("  recall, unseated                 {recall_unseated:?}");
            println!("  recall, seated (on, a page sunk) {recall_seated:?}");
        });
    }

    /// What the readers answered about the pages recall kept showing them,
    /// replayed in time order over the rows this seat has already written
    /// (`ZO_RERANK_REPLAY_LEDGER`), counted the way the product counts.
    ///
    /// The product folds a showing only from the label the turn's end wrote
    /// (`fold_shown`): a showing whose turn wrote no label was observed by
    /// nobody and counts neither way. So here a reading row is a showing of
    /// the first `MAX_RECALLED_ENTRIES` notes the turn read (`judged.recalled`,
    /// or `proposed` when applied), and it is folded when — and only as far
    /// as — its label says what became of it:
    ///
    /// * a row since t-6264 names every note shown (`shown`): complete;
    /// * an older row whose turn touched no note it was handed (`rank`
    ///   absent beside a mark, or `notCompared: no_note_touched`): complete,
    ///   every shown note unopened;
    /// * an older row that names the first note touched (`rank`), and whether
    ///   the judgment's first (`agreed`) and recall's first (`baselineAgreed`)
    ///   were: partial — those notes opened, every other note UNKNOWN and not
    ///   folded, because the row cannot say it went unopened.
    ///
    /// Each showing is judged on the demand folded from the labels BEFORE
    /// it, and folded after, so no later opening reaches an earlier rank.
    /// Reports what the product's rule (`RecallDemand::unaddressed`) would
    /// have sunk, whether a reader opened a page it sank, and the recall
    /// slot's agreement — the first slot opened — as recorded (before) and
    /// with sunk pages moved behind the rest (after), with the unknowns
    /// counted apart. The judgment's own mark (`agreed`) is not replayed: the
    /// demand changes what recall hands the judgment, and a row carries no
    /// query or summary to ask it again with. Beside the product's survival
    /// window, the same fold at others, as the constant comparison.
    ///
    /// ```text
    /// ZO_RERANK_REPLAY_LEDGER=/path/to/a/copy/of/rerank-shadow.jsonl \
    ///   cargo test -p tools --lib -- what_the_readers_answered --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "replays this machine's real ledger; run deliberately"]
    fn what_the_readers_answered_about_the_pages_recall_kept_showing() {
        use serde_json::Value;
        let ledger = std::env::var_os("ZO_RERANK_REPLAY_LEDGER")
            .map(PathBuf::from)
            .expect("point ZO_RERANK_REPLAY_LEDGER at a rerank-shadow.jsonl");
        let text = std::fs::read_to_string(&ledger).expect("a ledger");
        let mut rows: Vec<Value> = text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
        rows.sort_by_key(|row| row["at"].as_i64().unwrap_or(0));
        println!("\n  ledger: {} (fingerprint {:016x}, {} bytes, {} rows)", ledger.display(), task_fingerprint("", &text), text.len(), rows.len());
        for window in [1, 3, runtime::memory::recall::UNADDRESSED_AFTER_RECALLS, 10] {
            replay_at(&rows, window).print();
        }
    }

    /// What one population of showings came to, ranked the product's way.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    struct Population {
        /// Labeled showings.
        exposures: usize,
        /// Showings whose label says what became of every note shown.
        complete: usize,
        /// Showings whose label says it of some notes only.
        partial: usize,
        /// Showings the demand would have changed: a slot it sank.
        changed: usize,
        sunk_slots: usize,
        slots: usize,
        /// Opened notes the demand had sunk: readers answering the prior back.
        harmed: usize,
        /// Opened notes outside the shown slots, folded nowhere.
        outside: usize,
        /// Showings whose first slot the demand moved.
        moved: usize,
        /// First slot opened — yes, no, unknown — as recorded, and with the
        /// sunk pages behind the rest.
        before: [usize; 3],
        after: [usize; 3],
    }

    /// What one pass of the replay counted.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct Replayed {
        window: u32,
        /// Labels that name their own showing (`shown`, `shownAt`), each
        /// timed by its turn's first showing — a note a later request of the
        /// turn added is ranked at that time too, since the row says the
        /// turn saw it and not when.
        named: Population,
        /// Older labels on a run of one request that no earlier request of
        /// their reading may still have owed a label: that request was the
        /// showing, confirmed.
        confirmed: Population,
        /// Older labels on a run of several requests, or on a run of one an
        /// earlier request of their reading may still have owed a label,
        /// ranked at the run's first as though every run of the reading were
        /// one turn's — which a turn that was cancelled or failed, and wrote
        /// no label, or ran on past another turn's label, makes untrue. The
        /// assumption's replay, counted apart from the confirmed.
        conditional: Population,
        /// Runs of one reading's requests.
        bundles: usize,
        /// Requests folded into a run after its first.
        repeated: usize,
        /// Runs no label claimed.
        unlabeled: usize,
        /// Labels whose showing cannot be placed in time: an older label on a
        /// run another label claims too, or a named showing with no time.
        /// What they say is folded; they rank nothing.
        ambiguous: usize,
        /// Older labels on a run whose requests were judged in different
        /// orders: which page their marks name cannot be told, so nothing
        /// they say is folded, and they rank nothing.
        undetermined: usize,
        /// Older labels naming no reading written before them.
        orphan: usize,
        /// Older labels with no mark to read.
        unmarked: usize,
        /// Older showings whose time is uncertain: a record-only road writes
        /// the request row after the showing, by the judgment's own latency
        /// (`elapsed_ms`), and another label was written inside that latency
        /// — the rank read at the row may have seen it.
        unsure: usize,
        /// Older labels on a run of one request that an earlier run of their
        /// reading may still have owed a label — a turn of it running on past
        /// another's label: that turn's late label or the request's, which
        /// cannot be told, so none is confirmed.
        crossed: usize,
        /// Pages unaddressed at the end, of those observed.
        unaddressed: usize,
        observed: usize,
    }

    impl Replayed {
        fn print(&self) {
            let share = |part: usize, whole: usize| {
                #[expect(clippy::cast_precision_loss, reason = "a share of a few hundred showings")]
                let share = if whole == 0 { 0.0 } else { 100.0 * part as f64 / whole as f64 };
                share
            };
            let product = self.window == runtime::memory::recall::UNADDRESSED_AFTER_RECALLS;
            println!("\n  survival window {}{}", self.window, if product { "  <- the product's" } else { "" });
            println!(
                "    reading runs {} ({} repeated requests folded in); unlabeled runs {}; labels that cannot be placed {}; older labels whose run was judged in two orders {}, joining no reading {}, with no mark {}",
                self.bundles, self.repeated, self.unlabeled, self.ambiguous, self.undetermined, self.orphan, self.unmarked
            );
            println!("    confirmed older showings a label may have come between the showing and its request row: {}", self.unsure);
            println!("    older labels on one request an earlier run of their reading may still have owed a label (not confirmed): {}", self.crossed);
            for (name, counted) in [
                ("named showings (at the turn's first showing)", &self.named),
                ("older labels, one request's showing (confirmed)", &self.confirmed),
                ("older labels, a run of requests taken for one turn's, or one request a run before may owe (conditional — an assumption, not a result)", &self.conditional),
            ] {
                println!("    {name}: {} (complete {}, partial {})", counted.exposures, counted.complete, counted.partial);
                println!(
                    "      showings the demand changed        {}/{} = {:5.1}%   slots sunk {}/{}",
                    counted.changed,
                    counted.exposures,
                    share(counted.changed, counted.exposures),
                    counted.sunk_slots,
                    counted.slots
                );
                println!("      opened notes the demand had sunk   {}   (readers answering the prior back)", counted.harmed);
                println!("      opened notes outside the shown     {}   (folded nowhere)", counted.outside);
                println!("      first slot moved                   {}/{}", counted.moved, counted.exposures);
                println!(
                    "      first slot opened — before: yes {} no {} unknown {}   after: yes {} no {} unknown {}",
                    counted.before[0], counted.before[1], counted.before[2], counted.after[0], counted.after[1], counted.after[2]
                );
            }
            println!("    pages unaddressed at the end         {} of {} observed", self.unaddressed, self.observed);
        }
    }

    /// On what footing a replayed showing is ranked.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Footing {
        /// A label that names its own showing, timed by its turn's first.
        Named,
        /// An older label on a run of one request, nothing earlier owed a
        /// label: that request was it.
        Confirmed,
        /// An older label on a run of several requests, or of one an earlier
        /// run may still have owed a label, taken for one turn's: an
        /// assumption, ranked apart.
        Conditional,
    }

    /// One pass of the replay at survival window `window` — the product's
    /// rule with its one number moved, so the comparison is the same fold.
    ///
    /// In time only: a showing is ranked on the demand folded from the labels
    /// written before it was shown, and a label is folded when it was
    /// written — so no reader's later answer reaches an earlier rank, however
    /// the turns overlapped (at one instant, the showing goes first). A label
    /// that names its own showing (`shown`, `shownAt`) is its own exposure,
    /// timed by its turn's first showing. An older label names its reading
    /// only by the two fingerprints, so it is joined to the run of requests
    /// of that reading written before it — and the run says only that its
    /// turn asked among them: a turn that was cancelled or failed asked too
    /// and wrote no label, so a run of several requests may be one turn's or
    /// several's, and which of them was the label's showing cannot be told
    /// (t-6264 r3). A run of one request is that showing, confirmed, timed by
    /// its row — which on the record-only road lands after the showing, by
    /// the judgment's latency, so the time is an upper bound and `unsure`
    /// counts the showings another label came inside it. A run of several is
    /// ranked only on the assumption that it was one turn's, at its first
    /// request, and apart (`conditional`); a run two labels claim is not
    /// ranked at all, and both labels are ambiguous. A label closes its run,
    /// but not every turn of it: a turn of the run may run on past another's
    /// label, and its own may come after the next run's request. So what a
    /// run's requests outnumber its labels by is carried to the reading's
    /// next run as owed — each later label answering one at most — and while
    /// any is owed, a label on the next run may be that turn's late label:
    /// no confirmed showing, and ranked only as the assumption (t-6264 r4).
    /// What an older label says is folded when every request it may answer
    /// agrees on what its marks point at — the notes shown, the judgment's
    /// order, recall's first — its run's and those of every run still owing,
    /// so it reads the same whichever request was its showing; requests
    /// judged in different orders leave the pages its marks name unknown
    /// (`undetermined`), and nothing it says is folded.
    #[expect(clippy::too_many_lines, reason = "one replay, read top to bottom")]
    fn replay_at(rows: &[serde_json::Value], window: u32) -> Replayed {
        use serde_json::Value;
        use std::collections::{BTreeMap, BTreeSet};
        /// What one request of a run showed, and the judgment it carried:
        /// what an older label's marks point into.
        #[derive(Clone, PartialEq)]
        struct Asked {
            shown: Vec<String>,
            recalled_first: Option<String>,
            proposed: Vec<String>,
        }
        /// One run of a reading's requests: the rows naming one reading, no
        /// label of it between them.
        struct Run {
            at: u64,
            /// How long before its first row the showing may have been: the
            /// judgment's latency on the record-only road; none on the apply
            /// road, which writes the row before the turn reads.
            latency: u64,
            /// What its requests asked, each distinct reading once.
            asked: Vec<Asked>,
            requests: usize,
            claims: usize,
            /// How many requests of the reading's earlier runs may still have
            /// owed a label when it began, and what those runs asked — any of
            /// their requests may be the one owed.
            owed: usize,
            owed_asked: Vec<Asked>,
        }
        /// What a label says of the notes it speaks of: `Some(opened)`
        /// where it says, `None` where it cannot.
        struct Outcome {
            shown: Vec<String>,
            known: Vec<Option<bool>>,
            whole: bool,
            outside: usize,
            /// When it was shown, for a showing that ranks, and on what
            /// footing.
            ranked: Option<(u64, Footing)>,
            /// For a joined older label: how long before `ranked` the showing
            /// may really have been.
            latency: u64,
        }
        /// What an older label's marks say of one request's reading: each
        /// shown note opened, not, or unknown; whether that is every note;
        /// and how many opened notes lie outside those shown.
        fn marks_read(asked: &Asked, row: &Value, rank: Option<usize>, untouched: bool) -> (Vec<Option<bool>>, bool, usize) {
            if untouched {
                return (vec![Some(false); asked.shown.len()], true, 0);
            }
            let mut touched: BTreeSet<&String> = rank.and_then(|rank| asked.proposed.get(rank)).into_iter().collect();
            let mut said: BTreeMap<&String, bool> = BTreeMap::new();
            if let (Some(first), Some(agreed)) = (asked.proposed.first(), row["agreed"].as_bool()) {
                said.insert(first, agreed);
            }
            if let (Some(first), Some(agreed)) = (asked.recalled_first.as_ref(), row["baselineAgreed"].as_bool()) {
                said.insert(first, agreed);
            }
            touched.extend(said.iter().filter(|(_, opened)| **opened).map(|(slug, _)| *slug));
            let outside = touched.iter().filter(|slug| !asked.shown.contains(slug)).count();
            let known = asked
                .shown
                .iter()
                .map(|slug| if touched.contains(slug) { Some(true) } else { said.get(slug).copied() })
                .collect();
            (known, false, outside)
        }
        let unaddressed = |tally: &BTreeMap<String, (u32, u32)>, slug: &str| {
            tally.get(slug).is_some_and(|&(recalled, opened)| recalled >= window && opened == 0)
        };
        let list = |judged: &Value, name: &str| -> Vec<String> {
            judged[name].as_array().map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        let at = |row: &Value| row["at"].as_u64().unwrap_or(0);
        let mut replayed = Replayed { window, ..Replayed::default() };
        let mut order: Vec<&Value> = rows.iter().collect();
        order.sort_by_key(|row| at(row));
        // The runs, and each label with the run it claims.
        let mut runs: Vec<Run> = Vec::new();
        let mut open: HashMap<(u64, u64), usize> = HashMap::new();
        let mut latest: HashMap<(u64, u64), usize> = HashMap::new();
        let mut labels: Vec<(&Value, Option<usize>)> = Vec::new();
        for row in order {
            let key = (row["query"].as_u64().unwrap_or(0), row["notes"].as_u64().unwrap_or(0));
            if let Some(judged) = row.get("judged").filter(|_| row.get("outcome").is_some()) {
                let applied = row["applied"].as_bool().unwrap_or(false);
                let (recalled, proposed) = (list(judged, "recalled"), list(judged, "proposed"));
                let shown = if applied { &proposed } else { &recalled }.iter().take(MAX_RECALLED_ENTRIES).cloned().collect();
                let asked = Asked { shown, recalled_first: recalled.first().cloned(), proposed };
                if let Some(&run) = open.get(&key) {
                    replayed.repeated += 1;
                    let run = &mut runs[run];
                    run.requests += 1;
                    if !run.asked.contains(&asked) {
                        run.asked.push(asked);
                    }
                    continue;
                }
                // What the reading's closed runs may still owe: each label
                // answers one request at least, and none it has no request for.
                let (owed, owed_asked) = latest.get(&key).map_or((0, Vec::new()), |&before| {
                    let before = &runs[before];
                    let owed = (before.owed + before.requests).saturating_sub(before.claims);
                    let mut owed_asked: Vec<Asked> = Vec::new();
                    if owed > 0 {
                        for reading in before.owed_asked.iter().chain(&before.asked) {
                            if !owed_asked.contains(reading) {
                                owed_asked.push(reading.clone());
                            }
                        }
                    }
                    (owed, owed_asked)
                });
                open.insert(key, runs.len());
                latest.insert(key, runs.len());
                let latency = if applied { 0 } else { row["elapsed_ms"].as_u64().unwrap_or(0) };
                runs.push(Run { at: at(row), latency, asked: vec![asked], requests: 1, claims: 0, owed, owed_asked });
                continue;
            }
            if row.get("label").is_none() {
                continue;
            }
            open.remove(&key);
            let run = latest.get(&key).copied();
            if let Some(run) = run {
                runs[run].claims += 1;
            }
            labels.push((row, run));
        }
        replayed.bundles = runs.len();
        replayed.unlabeled = runs.iter().filter(|run| run.claims == 0).count();
        // What each label says, and when its showing was.
        let mut outcomes: Vec<(u64, Outcome)> = Vec::new();
        for (row, run) in labels {
            if let Some(notes) = row.get("shown").and_then(Value::as_array).filter(|notes| !notes.is_empty()) {
                let unfinished = row["unfinished"].as_bool().unwrap_or(false);
                let (shown, known) = notes
                    .iter()
                    .filter_map(|note| {
                        let opened = note["read"].as_bool().unwrap_or(false) || note["cited"].as_bool().unwrap_or(false);
                        let known = if opened { Some(true) } else { (!unfinished).then_some(false) };
                        note["slug"].as_str().map(|slug| (slug.to_string(), known))
                    })
                    .unzip();
                let ranked = row["shownAt"].as_u64().map(|shown_at| (shown_at, Footing::Named));
                replayed.ambiguous += usize::from(ranked.is_none());
                outcomes.push((at(row), Outcome { shown, known, whole: !unfinished, outside: 0, ranked, latency: 0 }));
                continue;
            }
            let Some(run) = run.map(|run| &runs[run]) else {
                replayed.orphan += 1;
                continue;
            };
            let rank = row["rank"].as_u64().and_then(|rank| usize::try_from(rank).ok());
            let marked = row["agreed"].as_bool().is_some();
            let untouched = row["notCompared"].as_str() == Some(NO_NOTE_TOUCHED) || (marked && rank.is_none());
            if !untouched && rank.is_none() {
                replayed.unmarked += 1;
                continue;
            }
            replayed.crossed += usize::from((run.claims, run.requests) == (1, 1) && run.owed > 0);
            // What the marks say under each reading the label may answer —
            // its run's and the owing runs' — one answer, or none that can be
            // told.
            let readings: Vec<_> = run
                .owed_asked
                .iter()
                .chain(&run.asked)
                .map(|asked| (&asked.shown, marks_read(asked, row, rank, untouched)))
                .collect();
            if readings.windows(2).any(|pair| pair[0] != pair[1]) {
                replayed.undetermined += 1;
                continue;
            }
            let Some((shown, (known, whole, outside))) = readings.into_iter().next() else {
                continue;
            };
            let ranked = match (run.claims, run.requests, run.owed) {
                (1, 1, 0) => Some((run.at, Footing::Confirmed)),
                (1, _, _) => Some((run.at, Footing::Conditional)),
                _ => None,
            };
            replayed.ambiguous += usize::from(ranked.is_none());
            outcomes.push((at(row), Outcome { shown: shown.clone(), known, whole, outside, ranked, latency: run.latency }));
        }
        replayed.unsure = outcomes
            .iter()
            .enumerate()
            .filter(|(_, (_, outcome))| outcome.latency > 0)
            .filter_map(|(index, (_, outcome))| match outcome.ranked {
                Some((shown_at, Footing::Confirmed)) => Some((index, shown_at, outcome.latency)),
                _ => None,
            })
            .filter(|&(index, shown_at, latency)| {
                outcomes
                    .iter()
                    .enumerate()
                    .any(|(other, (written, _))| other != index && (shown_at.saturating_sub(latency)..shown_at).contains(written))
            })
            .count();
        // In time: a showing ranks on what was folded before it; at one
        // instant the showing goes first.
        let mut events: Vec<(u64, bool, usize)> = Vec::new();
        for (index, (written, outcome)) in outcomes.iter().enumerate() {
            if let Some((shown_at, _)) = outcome.ranked {
                events.push((shown_at, false, index));
            }
            events.push((*written, true, index));
        }
        events.sort_unstable();
        let mut tally: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for (_, fold, index) in events {
            let Outcome { shown, known, whole, outside, ranked, .. } = &outcomes[index].1;
            if fold {
                // Each note once, and only as far as the label says.
                let mut named_once: BTreeSet<&String> = BTreeSet::new();
                for (slug, known) in shown.iter().zip(known) {
                    if let (true, Some(opened)) = (named_once.insert(slug), known) {
                        let counted = tally.entry(slug.clone()).or_default();
                        counted.0 += 1;
                        counted.1 += u32::from(*opened);
                    }
                }
                continue;
            }
            let Some((_, footing)) = ranked else {
                continue;
            };
            let counted = match footing {
                Footing::Named => &mut replayed.named,
                Footing::Confirmed => &mut replayed.confirmed,
                Footing::Conditional => &mut replayed.conditional,
            };
            counted.exposures += 1;
            if *whole {
                counted.complete += 1;
            } else {
                counted.partial += 1;
            }
            counted.outside += outside;
            counted.slots += shown.len();
            let sunk: Vec<bool> = shown.iter().map(|slug| unaddressed(&tally, slug)).collect();
            let sunk_here = sunk.iter().filter(|sunk| **sunk).count();
            counted.sunk_slots += sunk_here;
            counted.changed += usize::from(sunk_here > 0);
            counted.harmed += known.iter().zip(&sunk).filter(|(known, sunk)| **sunk && **known == Some(true)).count();
            let before_first = (!shown.is_empty()).then_some(0);
            let after_first = sunk.iter().position(|sunk| !sunk).or(before_first);
            counted.moved += usize::from(after_first != before_first);
            let tick = |counts: &mut [usize; 3], at: Option<usize>| {
                let slot = match at.and_then(|at| known[at]) {
                    Some(true) => 0,
                    Some(false) => 1,
                    None => 2,
                };
                counts[slot] += 1;
            };
            tick(&mut counted.before, before_first);
            tick(&mut counted.after, after_first);
        }
        replayed.unaddressed = tally.keys().filter(|slug| unaddressed(&tally, slug)).count();
        replayed.observed = tally.len();
        replayed
    }
}
