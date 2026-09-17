//! The memory rerank in shadow: every recall a turn performs is put to a System
//! One judgment off the turn's thread, and what that judgment would have
//! reordered is written beside what recall chose. The turn reads exactly what
//! it would have read with the shadow off.
//!
//! The question, the checks on the reply, and the rule that the vault's graph
//! outranks the judgment all belong to `runtime::memory::rerank`. This file owns
//! only what a shadow needs to run: the seat beside recall ([`RerankShadow`]),
//! the setting that permits it, the detached call, the memo, and the ledger row
//! — the same shape as the routing shadow next door (`decision_shadow.rs`), so a
//! reader of one can read the other.
//!
//! Putting a recall's notes to the judgment sends the vault's own summaries off
//! the machine. That is a different thing to consent to than the routing
//! shadow's task text, so it has its own switch, `smart.rerankShadow`, and is
//! off unless a person writes one of its record-only modes (`shadow`, `auto`).
//! Each request then goes through the Jev door (`jev_gate`), as the routing
//! shadow's do: consent, budget, withheld lines and caps.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{SystemOneClient, SystemOneConfig, SystemOneFailure, SystemOneRequest, SYSTEMONE_MODEL};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::RECALL;
use runtime::memory::rerank::{
    compare, rerank_candidates, rerank_questions, rerank_state, validate_rerank, RerankComparison,
    RerankReading, RERANK_RUBRIC_VERSION,
};
use runtime::{MemoryHit, RecallObserver};
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

/// The judgment's deadline. The same as the probe's: a recall's notes are a
/// smaller state than a task's text, and nothing waits on this either way.
pub const RERANK_SHADOW_DEADLINE: Duration = PROBE_TIMEOUT;

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
    /// What the judgment said, once it checked out and could be ordered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judged: Option<Judged>,
}

/// A checked judgment folded into recall's order, and the readings it came
/// from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judged {
    /// Recall's order — what the turn read.
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
            judged: None,
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
/// the setting and the ledger are; it decides per recall whether the setting
/// permits a judgment, and does so on the detached task, never on recall's
/// thread.
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

impl RecallObserver for RerankShadow {
    fn observe(&self, query: &str, hits: &[MemoryHit]) {
        fire(&self.cwd, query, hits);
    }
}

/// Everything one recall's shadow carries off the calling thread.
struct Shot {
    settings: runtime::ConfigLoader,
    cwd: PathBuf,
    ledger: PathBuf,
    config: Result<SystemOneConfig, SystemOneFailure>,
    query: String,
    hits: Vec<MemoryHit>,
}

/// Put one recall to the judgment, detached, and hand back its task. `None`
/// when there is nothing to judge or when an ablation holds the shadow out.
pub(super) fn fire(cwd: &Path, query: &str, hits: &[MemoryHit]) -> Option<tokio::task::JoinHandle<()>> {
    if hits.is_empty() || query.trim().is_empty() {
        return None;
    }
    if telemetry::attest_ablated(telemetry::HarnessFeature::RerankShadow) {
        return None;
    }
    let shot = Shot {
        settings: runtime::ConfigLoader::default_for(cwd),
        cwd: cwd.to_path_buf(),
        ledger: rerank_shadow_path(cwd),
        config: SystemOneConfig::from_env(),
        query: query.to_string(),
        hits: hits.to_vec(),
    };
    Some(shared_agent_runtime().spawn(run(shot)))
}

/// Read the setting, open the door, judge the reading, write the row.
async fn run(shot: Shot) {
    let Shot { settings, cwd, ledger, config, query, hits } = shot;
    // The door opens only for a mode that asks.
    let read = tokio::task::spawn_blocking(move || {
        rerank_shadow_mode_from(&settings).map(|mode| (mode, mode.asks().then(|| JevDoor::open(&cwd))))
    })
    .await
    .ok()
    .flatten();
    let Some((mode, door)) = read else {
        telemetry::attest_failed(telemetry::HarnessFeature::RerankShadow, FAIL_SETTINGS_UNAVAILABLE);
        return;
    };
    let Some(door) = door else {
        telemetry::attest_declined(telemetry::HarnessFeature::RerankShadow, mode.key());
        return;
    };
    let client = config.ok().map(SystemOneConfig::into_client);
    let row = judge(&door, client.as_ref(), &query, &hits, RERANK_SHADOW_DEADLINE).await;
    let _ = tokio::task::spawn_blocking(move || append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES)).await;
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
            if let Ok(readings) = validate_rerank(&candidates, &response) {
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
                row.model = Some(response.model);
                row.input_tokens = Some(response.usage.input_tokens);
                row.judged = judged;
                row
            } else {
                telemetry::attest_failed(telemetry::HarnessFeature::RerankShadow, SystemOneFailure::Schema.token());
                let mut row = RerankShadowRow::new(key, hits.len(), SystemOneFailure::Schema.ledger_token());
                // A response that arrived and failed its checks still billed.
                row.model = Some(response.model);
                row.input_tokens = Some(response.usage.input_tokens);
                row
            }
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
    fn nothing_to_judge_fires_nothing() {
        let dir = tempfile::tempdir().expect("a temp cwd");
        assert!(fire(dir.path(), "a query", &[]).is_none());
        assert!(fire(dir.path(), "   ", &[hit("wiki/a", "one")]).is_none());
    }
}
