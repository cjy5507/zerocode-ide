//! The compaction seat: every tool result a full compaction is about to
//! summarize away put to a System One judgment — does the remaining work
//! still need it? — and the ones it does not taken out of the summary's
//! input (t-6039).
//!
//! The question, the shards, the checks on a reply, the drop lean and the
//! plan surgery belong to `runtime::compaction_relevance`. This file owns
//! only what running it needs — the setting, the door, the shards on the
//! wire at the same time, the row and the labels — the same shape as the
//! skill seat next door (`skill_search.rs`), so a reader of one can read the
//! other.
//!
//! Putting the compaction set to the judgment sends the head of the person's
//! last request, the head of the assistant's newest words, and for each
//! block its tool's name and the heads of its input and output. That is a
//! different thing to consent to than a task's text or the vault's
//! summaries, so it has its own switch, `smart.jevCompaction`, and is off
//! unless a person writes one of its other words. Each request then goes
//! through the Jev door (`jev_gate`): consent, budget, withheld lines and
//! caps.
//!
//! # Nothing gets worse for asking
//!
//! A shard the door refuses, that fails, that misses the wall, or whose reply
//! breaks the contract keeps every block it covered, and the row says which
//! reader the summary's input came from (`routeUse`). Under a recording mode
//! the row carries what would have been dropped and the summary reads what
//! it read before.
//!
//! # The label is hindsight
//!
//! One label row per dropped block, written by [`note_compaction_reread`] at
//! a turn's end: `agreed: false` the turn a dropped block's path was read
//! again or its call made again — the seat's regret — and `agreed: true`
//! once [`COMPACTION_REGRET_TURNS`] turns have passed without either. The
//! judge counts those rows as this seat's agreement
//! (`zerocode_core::jev::summary::AGREED`), which is what `auto` rises on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use api::{
    SystemOneCall, SystemOneClient, SystemOneConfig, SystemOneFailure, SystemOneRequest,
    SYSTEMONE_MODEL,
};
use futures_util::future::BoxFuture;
use runtime::compaction_relevance::{
    questions, shards, state, validate, BlockReading, CompactionRejection,
};
use runtime::{BlockHead, CompactionAsk, CompactionJudgment, CompactionSeat, ConversationMessage, ContentBlock, MessageRole, COMPACTION_RUBRIC_VERSION};
use serde::{Deserialize, Serialize};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::promote;
use zerocode_core::jev::{
    JevMode, COMPACTION, COMPACTION_REGRET_TURNS, ROUTE_USE_APPLIED, ROUTE_USE_FALLBACK,
};

use super::jev_gate::{self, JevDoor};
use super::probe_exec::task_fingerprint;
use super::settings::jev_compaction_mode_from;
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};
use super::turn_reads::read_path;

/// The seat's ledger file — the Jev use table's name for it.
pub const COMPACTION_RELEVANCE_FILE: &str = COMPACTION.ledger;

/// Outcome of a row whose judgment answered and checked out — the door's
/// word, because the one counter every seat shares reads it.
pub const COMPACTION_OUTCOME_ANSWERED: &str = zerocode_core::jev::door::ANSWERED_OUTCOME;

/// The wall the whole batch waits — every shard leaves together and the
/// slowest one is what the boundary sits through. The use table's own
/// number, so the stage that waits and the judge that reads the wait cannot
/// disagree.
pub const COMPACTION_JUDGMENT_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::COMPACTION_APPLY_DEADLINE_MS);
const _: () = assert!(
    matches!(
        COMPACTION.apply_deadline_ms,
        Some(zerocode_core::jev::COMPACTION_APPLY_DEADLINE_MS)
    ),
    "the seat's row names the wall its stage waits"
);

/// The word a label row carries as its kind.
pub const LABEL_ROW_KIND: &str = "label";

const FAIL_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

/// Where a project's compaction ledger lives.
#[must_use]
pub fn compaction_relevance_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, COMPACTION_RELEVANCE_FILE)
}

/// One compaction's row: what was asked, what came back, what left the
/// summary's input, and which reader the input came from.
///
/// No goal, no words, no heads: the goal is a fingerprint, and a block is
/// counted, never named. The blocks that were dropped live in memory for
/// the label ([`note_compaction_reread`]), not on the row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionRow {
    /// Unix milliseconds when the row was made.
    pub at: u64,
    /// The turn the compaction ran inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    /// Fingerprint of the goal and the block ids judged — what a label row
    /// names under `label`.
    pub judged: u64,
    pub rubric_version: u32,
    /// [`COMPACTION_OUTCOME_ANSWERED`], a failure's ledger token, or the
    /// door's refusal token.
    pub outcome: String,
    /// How many blocks were put to the judgment.
    pub candidates: usize,
    /// How many requests they were cut into, and how many came back checked.
    pub shards: usize,
    pub shards_answered: usize,
    /// Blocks the judgment would drop, past the table's lean.
    pub dropped: usize,
    /// Bytes those blocks' bodies hold — what a drop takes off the summary's
    /// input, before the request's own pre-trim.
    pub dropped_bytes: usize,
    /// Which reader the summary's input came from: the judgment
    /// ([`ROUTE_USE_APPLIED`]), a recording mode's word, or the plan as
    /// prepared ([`ROUTE_USE_FALLBACK`]).
    pub route_use: String,
    /// Whether the dropped blocks actually left the summary's input.
    pub applied: bool,
    #[serde(default)]
    pub cached: bool,
    pub elapsed_ms: u64,
    pub retries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Requests this compaction sent: none when the door refused it, one per
    /// shard plus their retries when they left.
    pub requests: u32,
    /// Lines the door withheld from what was sent.
    pub redacted_lines: u32,
    /// Which of a reply's rules refused it, on a row whose `outcome` is
    /// `schema`, and which block it broke on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_at: Option<usize>,
}

impl CompactionRow {
    fn new(ask: &CompactionAsk, shards: usize, outcome: String) -> Self {
        Self {
            at: super::decision_shadow::unix_millis(),
            attempt: Some(ask.attempt.trim())
                .filter(|attempt| !attempt.is_empty())
                .map(str::to_string),
            judged: judged_key(ask),
            rubric_version: COMPACTION_RUBRIC_VERSION,
            outcome,
            candidates: ask.blocks.len(),
            shards,
            shards_answered: 0,
            dropped: 0,
            dropped_bytes: 0,
            route_use: ROUTE_USE_FALLBACK.to_string(),
            applied: false,
            cached: false,
            elapsed_ms: 0,
            retries: 0,
            model: None,
            input_tokens: None,
            requests: 0,
            redacted_lines: 0,
            rejected: None,
            rejected_at: None,
        }
    }
}

/// The fingerprint a compaction's row and its labels share: the goal's
/// words and the ids of every block judged.
fn judged_key(ask: &CompactionAsk) -> u64 {
    let ids: Vec<&str> = ask.blocks.iter().map(|block| block.tool_use_id.as_str()).collect();
    task_fingerprint(&ask.goal, &ids.join("\u{1f}"))
}

/// One dropped block's hindsight, as the ledger keeps it: whether the turns
/// after the compaction went back for it. Shaped like the recall seat's label
/// (`rerank_shadow::RerankLabelRow`): the row it grades under `label`, the
/// mark under `agreed`, and `applied` from the row so an applied drop and a
/// recorded one are compared on the same mark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionLabelRow {
    pub kind: String,
    pub at: u64,
    /// The row this grades — its `judged` fingerprint, spelled as text.
    pub label: String,
    /// Fingerprint of the dropped block's call (its tool and input).
    pub block: u64,
    pub tool: String,
    pub applied: bool,
    /// `false` is regret: the block was read again inside the window.
    pub agreed: bool,
    /// Turns after the compaction at which the label was decided.
    pub turns_later: u32,
}

/// The seat a host installs on the runtime: asked on the runtime's own task
/// at the compaction boundary, answered inside the row's wall.
#[derive(Debug)]
pub struct CompactionJudge {
    cwd: PathBuf,
}

impl CompactionJudge {
    /// The seat for a project — its setting and its ledger live under the
    /// project's working directory.
    #[must_use]
    pub fn at(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
        }
    }
}

impl CompactionSeat for CompactionJudge {
    fn judge<'a>(&'a self, ask: &'a CompactionAsk) -> BoxFuture<'a, CompactionJudgment> {
        Box::pin(judge_at(&self.cwd, ask))
    }
}

/// The mode this compaction is to be judged under, or `None` when it is not
/// to be judged at all: an ablation holding it out, an unreadable setting,
/// or a mode that asks nothing.
fn asking_mode(cwd: &Path) -> Option<JevMode> {
    if telemetry::attest_ablated(telemetry::HarnessFeature::DecisionShadow) {
        return None;
    }
    let Some(mode) = jev_compaction_mode_from(&runtime::ConfigLoader::default_for(cwd)) else {
        telemetry::attest_failed(telemetry::HarnessFeature::DecisionShadow, FAIL_SETTINGS_UNAVAILABLE);
        return None;
    };
    if !mode.asks() {
        telemetry::attest_declined(telemetry::HarnessFeature::DecisionShadow, mode.key());
        return None;
    }
    Some(mode)
}

/// Ask about one compaction on the road this project's setting names, write
/// the row, and remember what was dropped for the label.
async fn judge_at(cwd: &Path, ask: &CompactionAsk) -> CompactionJudgment {
    let Some(mode) = asking_mode(cwd) else {
        return CompactionJudgment::default();
    };
    // Read once, here: the standing decides whether the dropped blocks leave
    // the summary's input, and two readings of one ledger could answer that
    // twice.
    let acting = mode.applies_with(runtime::jev_seat_applies(cwd, &COMPACTION));
    let opened_at = cwd.to_path_buf();
    let Ok(door) = tokio::task::spawn_blocking(move || JevDoor::open(&opened_at)).await else {
        telemetry::attest_failed(telemetry::HarnessFeature::DecisionShadow, FAIL_SETTINGS_UNAVAILABLE);
        return CompactionJudgment::default();
    };
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    let (row, dropped) = judge(&door, client.as_ref(), ask, acting).await;
    let (row, judgment) = settle(mode, acting, row, dropped);
    let ledger = compaction_relevance_path(cwd);
    if !judgment.dropped.is_empty() {
        remember_dropped(cwd, &row, ask, &judgment.dropped);
    }
    let _ = tokio::task::spawn_blocking(move || {
        let written = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES);
        let _ = judge_ledger(&ledger, super::decision_shadow::now_ms());
        written
    })
    .await;
    judgment
}

/// What the row says the summary's input came from, and what the runtime is
/// told: the judgment's when it answered and the mode acts — even one that
/// dropped nothing, since the summary then reads what the judgment kept —
/// the mode's own word when it answered and only records, and the plan as
/// prepared when nothing answered. A recording mode still hands the drops
/// back, so the label can grade them; the runtime applies none of them.
fn settle(
    mode: JevMode,
    acting: bool,
    mut row: CompactionRow,
    dropped: Vec<usize>,
) -> (CompactionRow, CompactionJudgment) {
    let answered = row.outcome == COMPACTION_OUTCOME_ANSWERED;
    row.applied = answered && acting;
    row.route_use = if row.applied {
        ROUTE_USE_APPLIED.to_string()
    } else if answered {
        mode.key().to_string()
    } else {
        ROUTE_USE_FALLBACK.to_string()
    };
    let judgment = CompactionJudgment {
        dropped,
        applies: row.applied,
    };
    (row, judgment)
}

/// What one shard's request came back with.
struct Shard {
    readings: Result<Vec<BlockReading>, Refusal>,
    call: Option<SystemOneCall>,
    withheld: u32,
}

/// Why a shard produced no readings, in the words a ledger row keeps.
struct Refusal {
    outcome: String,
    rejected: Option<String>,
    rejected_at: Option<usize>,
}

/// One compaction's row and the positions it would drop: refused at the
/// door, or asked in shards and checked. Writes nothing.
pub(super) async fn judge(
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    ask: &CompactionAsk,
    acting: bool,
) -> (CompactionRow, Vec<usize>) {
    let cut = shards(&ask.blocks);
    // Every shard at once: they are independent requests over disjoint
    // blocks, and the boundary waits for the slowest of them either way.
    let asked = cut.iter().map(|shard| ask_one(door, client, ask, shard, acting));
    let answers = futures_util::future::join_all(asked).await;
    fold(ask, cut.len(), answers)
}

/// Ask one shard, through the door.
async fn ask_one(
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    ask: &CompactionAsk,
    shard: &[BlockHead],
    acting: bool,
) -> Shard {
    let refused = |outcome: String| Shard {
        readings: Err(Refusal {
            outcome,
            rejected: None,
            rejected_at: None,
        }),
        call: None,
        withheld: 0,
    };
    let state = state(ask, shard);
    let questions = questions(shard);
    let request = SystemOneRequest {
        state: &state,
        model: SYSTEMONE_MODEL,
        questions: &questions,
    };
    let Some(body) = jev_gate::body_of(&request) else {
        let failure = SystemOneFailure::InvalidRequest;
        telemetry::attest_failed(telemetry::HarnessFeature::DecisionShadow, failure.token());
        return refused(failure.ledger_token());
    };
    let (cleared, client) = match (door.pass(&COMPACTION, client.is_some(), body), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        (passed, _) => {
            let refusal = passed.err().unwrap_or(Refused::NoKey);
            telemetry::attest_declined(telemetry::HarnessFeature::DecisionShadow, refusal.token());
            return refused(refusal.token().to_string());
        }
    };
    let withheld = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    // A hedge buys an answer inside a wall, and this seat is waited inside
    // one only when it acts: a recording compaction is written down beside
    // what the summary read anyway.
    let hedge = door.hedge_now(&COMPACTION, COMPACTION_JUDGMENT_DEADLINE, acting);
    let call = jev_gate::send(client, cleared, COMPACTION_JUDGMENT_DEADLINE, hedge).await;
    let readings = match &call.outcome {
        Ok(response) => validate(shard, response).map_err(|rejection: CompactionRejection| Refusal {
            outcome: SystemOneFailure::Schema.ledger_token(),
            rejected: Some(rejection.rule().to_string()),
            rejected_at: rejection.position(),
        }),
        Err(failure) => Err(Refusal {
            outcome: failure.ledger_token(),
            rejected: None,
            rejected_at: None,
        }),
    };
    Shard {
        readings,
        call: Some(call),
        withheld,
    }
}

/// Fold every shard's answer into one row and one drop list.
///
/// A shard that was refused keeps its blocks and nothing else: the others
/// asked about different blocks and their answers stand. The row's outcome
/// is the compaction's — answered when anything was, and the first refusal's
/// word when nothing was.
fn fold(ask: &CompactionAsk, shards: usize, answers: Vec<Shard>) -> (CompactionRow, Vec<usize>) {
    let mut row = CompactionRow::new(ask, shards, String::new());
    let mut dropped = Vec::new();
    let mut first_refusal: Option<Refusal> = None;
    let (mut requests, mut withheld, mut input_tokens, mut elapsed, mut retries) = (0, 0, 0, 0, 0);
    for answer in answers {
        withheld += answer.withheld;
        if let Some(call) = &answer.call {
            requests += call.requests;
            retries = retries.max(call.retries);
            // The slowest shard is what the boundary waited: they left together.
            elapsed = elapsed.max(jev_gate::millis(call.elapsed));
            if let Ok(response) = &call.outcome {
                input_tokens += response.usage.input_tokens;
                row.model.get_or_insert_with(|| response.model.clone());
            }
        }
        match answer.readings {
            Ok(read) => {
                row.shards_answered += 1;
                dropped.extend(read.iter().filter(|reading| reading.drops()).map(|reading| reading.position));
            }
            Err(refusal) => {
                if first_refusal.is_none() {
                    first_refusal = Some(refusal);
                }
            }
        }
    }
    row.requests = requests;
    row.redacted_lines = withheld;
    row.input_tokens = (input_tokens > 0).then_some(input_tokens);
    row.elapsed_ms = elapsed;
    row.retries = retries;
    if row.shards_answered > 0 {
        telemetry::attest_fired(telemetry::HarnessFeature::DecisionShadow);
        row.outcome = COMPACTION_OUTCOME_ANSWERED.to_string();
        dropped.sort_unstable();
        row.dropped = dropped.len();
        row.dropped_bytes = ask
            .blocks
            .iter()
            .filter(|block| dropped.contains(&block.position))
            .map(|block| block.output_bytes)
            .sum();
        return (row, dropped);
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

/// Judge the seat on what it has just written, and write down a rise or a
/// fall — the one judge every seat that carries its own `agreed` marks
/// takes (`shadow_ledger::judge_seat_ledger`).
#[must_use]
pub fn judge_ledger(ledger: &Path, now_ms: i64) -> Option<promote::Verdict> {
    super::shadow_ledger::judge_seat_ledger(&COMPACTION, ledger, now_ms)
}

/* ---- the label: what the turns after went back for ------------------------- */

/// One dropped block, as the label waits on it: the call that produced it,
/// so a later turn making the same call or reading the same path is caught.
#[derive(Debug, Clone)]
struct Dropped {
    block: u64,
    tool: String,
    path: Option<String>,
}

/// One compaction's drops, waiting on the turns after it.
#[derive(Debug, Clone)]
struct Pending {
    label: String,
    applied: bool,
    turns_seen: u32,
    blocks: Vec<Dropped>,
}

type PendingBook = HashMap<PathBuf, Vec<Pending>>;

/// The drops still waiting on their window, per project. In memory and not
/// on disk, for the reason the other seats' books are: the mark is whether
/// THIS session's next turns went back for what THIS compaction dropped,
/// and a compaction read back off a ledger row could be another session's.
/// A process that ends before the window closes labels nothing, as a pane
/// placed before the window process started is not labeled.
fn pending() -> &'static Mutex<PendingBook> {
    static PENDING: OnceLock<Mutex<PendingBook>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The fingerprint a dropped block's call is matched by later: its tool and
/// its whole input.
fn call_key(tool: &str, input: &str) -> u64 {
    task_fingerprint(tool, input)
}

fn remember_dropped(cwd: &Path, row: &CompactionRow, ask: &CompactionAsk, dropped: &[usize]) {
    let blocks: Vec<Dropped> = ask
        .blocks
        .iter()
        .filter(|block| dropped.contains(&block.position))
        .map(|block| Dropped {
            block: call_key(&block.tool_name, &block.input),
            tool: block.tool_name.clone(),
            path: read_path(&block.tool_name, &block.input),
        })
        .collect();
    if blocks.is_empty() {
        return;
    }
    if let Ok(mut book) = pending().lock() {
        book.entry(cwd.to_path_buf()).or_default().push(Pending {
            label: row.judged.to_string(),
            applied: row.applied,
            turns_seen: 0,
            blocks,
        });
    }
}

/// What one turn went back for: every path a successful read named, and the
/// fingerprint of every call the assistant made.
fn went_back_for(turn: &[ConversationMessage]) -> (Vec<String>, Vec<u64>) {
    let mut asked: HashMap<&str, (String, Option<String>)> = HashMap::new();
    let mut calls = Vec::new();
    let mut read = Vec::new();
    for message in turn {
        for block in &message.blocks {
            match block {
                ContentBlock::ToolUse { id, name, input } if message.role == MessageRole::Assistant => {
                    calls.push(call_key(name, input));
                    asked.insert(id.as_str(), (name.clone(), read_path(name, input)));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => {
                    if let Some((_, Some(path))) = asked.remove(tool_use_id.as_str()) {
                        if !is_error {
                            read.push(path);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (read, calls)
}

/// Write this seat's hindsight for the turn that just ended, judged on
/// `turn` — the messages the turn appended, already in memory — against
/// every compaction of `cwd`'s still inside its window. A dropped block the
/// turn read again (its path, or its call) is labeled `agreed: false` now;
/// the rest are labeled `agreed: true` once [`COMPACTION_REGRET_TURNS`]
/// turns have passed. `None` is a cancelled turn, which is not a turn of the
/// window and writes nothing. Answers how many label rows were written.
#[must_use]
pub fn note_compaction_reread(cwd: &Path, turn: Option<&[ConversationMessage]>) -> usize {
    label_turn(cwd, &compaction_relevance_path(cwd), turn)
}

/// [`note_compaction_reread`], writing to `ledger` — the seam a test hands
/// a path of its own, so no row lands in the person's home.
fn label_turn(cwd: &Path, ledger: &Path, turn: Option<&[ConversationMessage]>) -> usize {
    let Some(turn) = turn else {
        return 0;
    };
    let Ok(mut book) = pending().lock() else {
        return 0;
    };
    let Some(waiting) = book.get_mut(cwd) else {
        return 0;
    };
    let (read, calls) = went_back_for(turn);
    let at = super::decision_shadow::unix_millis();
    let mut rows: Vec<CompactionLabelRow> = Vec::new();
    for compaction in waiting.iter_mut() {
        compaction.turns_seen += 1;
        let turns_later = compaction.turns_seen;
        let window_closed = turns_later >= COMPACTION_REGRET_TURNS;
        let mut still = Vec::new();
        for dropped in compaction.blocks.drain(..) {
            let regretted = calls.contains(&dropped.block)
                || dropped.path.as_ref().is_some_and(|path| read.contains(path));
            if regretted || window_closed {
                rows.push(CompactionLabelRow {
                    kind: LABEL_ROW_KIND.to_string(),
                    at,
                    label: compaction.label.clone(),
                    block: dropped.block,
                    tool: dropped.tool.clone(),
                    applied: compaction.applied,
                    agreed: !regretted,
                    turns_later,
                });
            } else {
                still.push(dropped);
            }
        }
        compaction.blocks = still;
    }
    waiting.retain(|compaction| !compaction.blocks.is_empty());
    if waiting.is_empty() {
        book.remove(cwd);
    }
    drop(book);
    if rows.is_empty() {
        return 0;
    }
    let written = rows
        .iter()
        .filter(|row| append_shadow_row(ledger, row, SHADOW_LEDGER_MAX_BYTES).is_ok())
        .count();
    let _ = judge_ledger(ledger, super::decision_shadow::now_ms());
    written
}

/// The book of pending drops for `cwd`, emptied — for a test that seeds it
/// by hand.
#[cfg(test)]
fn forget_pending(cwd: &Path) {
    if let Ok(mut book) = pending().lock() {
        book.remove(cwd);
    }
}

#[cfg(test)]
mod tests;
