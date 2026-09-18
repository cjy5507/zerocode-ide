//! The memory rerank: every recall a turn performs is put to a System One
//! judgment, and what that judgment would have reordered is written beside what
//! recall chose. Under `shadow` and `auto` that is all that happens and the turn
//! reads exactly what it would have read with the switch off; under `on` the
//! judgment's order — after the vault's graph has had its say — is the order the
//! turn reads.
//!
//! The question, the checks on the reply, the rule that the vault's graph
//! outranks the judgment, and the fold that proves an order a permutation before
//! it changes anything all belong to `runtime::memory::rerank`. This file owns
//! only what running it needs: the seat beside recall ([`RerankShadow`]), the
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
//! The apply road can only ever hand back recall's own order or a proved
//! permutation of it. A judgment that misses the wall, that the door refuses,
//! that fails its checks, or whose order cannot be proved a permutation leaves
//! recall's order standing, and the row says `applied: false` either way — so
//! the ledger can be read back for how often the switch actually moved
//! anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{SystemOneClient, SystemOneConfig, SystemOneFailure, SystemOneRequest, SYSTEMONE_MODEL};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::RECALL;
use runtime::memory::rerank::{
    apply_order, compare, rerank_candidates, rerank_questions, rerank_state, validate_rerank,
    RerankComparison, RerankReading, RERANK_RUBRIC_VERSION,
};
use runtime::{MemoryHit, RecallSeat};
use serde::{Deserialize, Serialize};

use super::jev_gate::{self, JevDoor};
use super::probe_exec::{remember_bounded, task_fingerprint, PROBE_TIMEOUT};
use super::settings::rerank_shadow_mode_from;
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};
use crate::misc_tools::agent_tools::shared_agent_runtime;

/// The rerank shadow's ledger file, under the shared shadow-ledger directory
/// — the Jev use table's name for the recall row's ledger.
pub const RERANK_SHADOW_FILE: &str = zerocode_core::jev::RECALL.ledger;

/// Outcome of a row whose judgment answered, checked out, and could be folded
/// into recall's order.
pub const RERANK_OUTCOME_ANSWERED: &str = "answered";

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
    /// `schema` — [`RerankRejection::rule`]'s word. One word for nine rules
    /// says a reply was refused but not by what, and the ledger is where the
    /// cause has to be readable. Never a word of the reply, the notes or the
    /// request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
    /// Which note the rule broke on, by its place in recall's order. Absent
    /// when the rule names no note of ours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_at: Option<usize>,
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
    /// The judgment's order after the graph's rules.
    pub proposed: Vec<String>,
    pub moved: usize,
    pub top_changed: bool,
    /// Notes the graph pinned against the judgment. The number a later phase
    /// reads before letting a judgment reorder anything for real.
    pub held_by_graph: Vec<String>,
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
            readings: readings
                .iter()
                .map(|reading| (reading.normalised, reading.confidence))
                .collect(),
        }
    }
}

impl RerankShadowRow {
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
/// and waiting for the order the turn reads ([`settle`]).
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
    if !mode.applies() {
        fire(cwd, query, &hits, RERANK_SHADOW_DEADLINE, None);
        return hits;
    }
    apply(cwd, query, hits)
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
        .and_then(|judged| apply_order(&hits, &judged.proposed));
    row.applied = read.is_some();
    let _ = append_shadow_row(&rerank_shadow_path(cwd), &row, SHADOW_LEDGER_MAX_BYTES);
    read.unwrap_or(hits)
}

/// Open the door, judge the reading, and leave the row with whoever writes it.
async fn run(shot: Shot, answer: Option<RowSender>) {
    let Shot { cwd, ledger, config, query, hits, deadline } = shot;
    let Ok(door) = tokio::task::spawn_blocking(move || JevDoor::open(&cwd)).await else {
        telemetry::attest_failed(telemetry::HarnessFeature::RerankShadow, FAIL_SETTINGS_UNAVAILABLE);
        return;
    };
    let client = config.ok().map(SystemOneConfig::into_client);
    let row = judge(&door, client.as_ref(), &query, &hits, deadline).await;
    let Some(row) = kept(row, answer).await else {
        return;
    };
    let _ = tokio::task::spawn_blocking(move || append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES)).await;
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
    let call = jev_gate::send(client, cleared, deadline).await;
    let mut row = match call.outcome {
        Ok(response) => {
            let checked = validate_rerank(&candidates, &response);
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
            row.model = Some(response.model);
            row.input_tokens = Some(response.usage.input_tokens);
            row
        }
        Err(failure) => {
            telemetry::attest_failed(telemetry::HarnessFeature::RerankShadow, failure.token());
            RerankShadowRow::new(key, hits.len(), failure.ledger_token())
        }
    };
    row.elapsed_ms = u64::try_from(call.elapsed.as_millis()).unwrap_or(u64::MAX);
    row.retries = call.retries;
    row.requests = Some(call.retries.saturating_add(1));
    row.redacted_lines = Some(withheld);
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
        shared_agent_runtime().block_on(judge(&door, Some(client), query, hits, Duration::from_secs(5)))
    }

    #[test]
    fn an_answered_reading_writes_recalls_order_beside_the_judgments_and_what_the_graph_held() {
        let mock = Mock::serving(200, reply_for(&[1, 3, 0]));
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
        let mock = Mock::serving(200, reply_for(&[0, 3, 1]));
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
            assert!(
                rows[0].judged.as_ref().expect("a judgment").top_changed,
                "{} still asked, and the judgment still disagreed",
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
            readings: vec![(0.0, 0.9); 3],
        });

        let read = row
            .judged
            .as_ref()
            .and_then(|judged| apply_order(&hits, &judged.proposed));

        assert_eq!(read, None, "a note recall never admitted folds nothing");
    }
}
