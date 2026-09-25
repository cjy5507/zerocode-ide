//! The question search's asking stage (t-6349,
//! `tools/question-discovery/README.md`): every candidate question of one
//! round asked in ONE request per row, about the state a seat already sends,
//! through that seat's own row of the use table and a door of the search's
//! own — and written down as numbers beside each row's label and its cheapest
//! baselines.
//!
//! Nothing here chooses a question, fits a model or reads a held-out label:
//! that loop is `tools/question-discovery/loop.py`, which writes a round file
//! and reads the rows this stage writes. What a row is, what its state
//! carries and what its label says are the shipped functions' — the patch
//! review's `ask_reading`/`state`/`hindsight_of_turn`
//! (`replay_support::replay_points`), the notify seat's `notify_call::ask`
//! and `notify_call::agreed` — handed the same transcripts and seed rows the
//! seats' own replays read. The door is the seat's row (`PATCH_REVIEW`,
//! `NOTIFY`): the state leaves cut and cleared exactly as the seat sends it.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Duration;

use api::{SystemOneClient, SystemOneQuestion, SYSTEMONE_MODEL};
use runtime::patch_review::{unified_diff, TaskReading, Verdict};
use serde_json::{json, Value};
use zerocode_core::jev::door::{self, Asking, JevSettings};
use zerocode_core::jev::summary::percentile;
use zerocode_core::jev::{fingerprint_of, JevUse, NOTIFY, PATCH_REVIEW};
use zerocode_core::notify;
use zerocode_core::notify_call::{self, Attendance, Call, NotifyAsk, NotifyLook, Recent};

use super::jev_gate::{self, JevDoor};
use super::replay_support;

/// The round file `tools/question-discovery/loop.py` wrote.
const ROUND_ENV: &str = "ZEROCODE_QUESTION_DISCOVERY_ROUND";
/// Where the rows go, one JSON line each, a summary line last.
const OUT_ENV: &str = "ZEROCODE_QUESTION_DISCOVERY_OUT";
/// Where each row's state goes AS THE DOOR CLEARED IT — the bytes the wire
/// saw — for the proposer's eyes only. Unset, nothing of the kind is written.
const STATES_ENV: &str = "ZEROCODE_QUESTION_DISCOVERY_STATES";
/// The most requests one run may send, over [`REQUEST_CAP`].
const CAP_ENV: &str = "ZEROCODE_QUESTION_DISCOVERY_CAP";
/// The loop's remaining Jev allowance for this subprocess.
const SPEND_ENV: &str = "ZEROCODE_QUESTION_DISCOVERY_SPEND_CAP_USD";
/// The brief's line: three hundred requests a run, whatever the round asks.
const REQUEST_CAP: usize = 300;
/// The brief's other line, in dollars, so a runaway round stops on money
/// as well as on count.
const SPEND_CAP_USD: f64 = 4.0;
/// Conservative headroom above UTF-8 body bytes for server-side framing.
/// If observed usage exceeds this reservation, the study stops for audit.
const TOKEN_RESERVE_OVERHEAD: u64 = 8_192;
/// The wall each request waits: the chat probe's, so a slow answer is
/// measured rather than cut.
const WALL: Duration = super::probe_exec::PROBE_TIMEOUT;
/// The workspace the search's own door consents to.
const WORKSPACE: &str = "/work/question-discovery";
/// The word a round writes to ask the seat's own questions — the search's
/// "today's question" baseline.
const SHIPPED: &str = "shipped";
/// What a row's outcome says when the cap left it unasked.
const CAPPED: &str = "capped";
/// What a row's outcome says when every question came back readable.
const ANSWERED: &str = "answered";

/// What the loop wrote for one round.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Round {
    /// The seat's id in the use table.
    pub(super) seat: String,
    /// The seat's own replay seed: `tools/patch-review-replay/seed.py`'s for
    /// the patch review, `tools/notify-replay/seed.py`'s for the notify seat.
    pub(super) source: String,
    /// How many labeled rows to take, spread evenly over time; every one
    /// when unset.
    pub(super) sample: Option<usize>,
    /// Only these rows, by id, when set — a round asked of the sample an
    /// earlier round enumerated.
    pub(super) rows: Option<Vec<String>>,
    /// `"shipped"` for the seat's own questions, or a map of question id to
    /// its words: `{"type": "noul", "instructions", "yes", "no"}`,
    /// `{"type": "score", "instructions", "levels": [..]}`,
    /// `{"type": "choice", "instructions", "options": {name: meaning}}`. An
    /// empty map asks nothing and only writes the rows down.
    pub(super) questions: Value,
}

/// The seats the search can ask about: the ones whose state a replay can
/// rebuild at the judgment's own clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seat {
    PatchReview,
    Notify,
}

impl Seat {
    fn named(word: &str) -> Option<Self> {
        if word == PATCH_REVIEW.id {
            Some(Self::PatchReview)
        } else if word == NOTIFY.id {
            Some(Self::Notify)
        } else {
            None
        }
    }

    /// The row of the use table the door clears every request by.
    const fn row(self) -> &'static JevUse {
        match self {
            Self::PatchReview => &PATCH_REVIEW,
            Self::Notify => &NOTIFY,
        }
    }
}

/// One labeled row a seat could be asked about, with the state the seat
/// itself would send and the seat's own questions.
struct Row {
    id: String,
    /// When, in milliseconds — the order the loop splits by.
    at: u64,
    /// Order inside one moment (a transcript's message index, a seed's row).
    seq: usize,
    /// The bundle a row must not be split from — one transcript's patches,
    /// one pane's rings — so a split or a fold never learns a session on
    /// one side and is judged on it on the other. An opaque word: a
    /// transcript's index, a pane name's fingerprint.
    group: String,
    /// The bad outcome: a regretted patch, a ring the person turned to.
    label: bool,
    label_word: &'static str,
    /// What code knows without asking: the cheapest readers' inputs.
    baseline: Value,
    state: Value,
    /// The seat's own questions object, as the seat builds it.
    shipped: Value,
}

fn count_lines(hunks: &[runtime::StructuredPatchHunk], sign: char) -> usize {
    hunks.iter().flat_map(|hunk| &hunk.lines).filter(|line| line.starts_with(sign)).count()
}

/// Every labeled patch of the seed's transcripts: a patch whose window
/// closed inside its transcript. An open one has no label and is no row.
fn patch_rows(seed: &Value) -> (Vec<Row>, usize, usize, usize) {
    let (mut rows, mut read, mut skipped, mut unlabeled) = (Vec::new(), 0, 0, 0);
    for (transcript, entry) in seed["transcripts"].as_array().into_iter().flatten().enumerate() {
        let Some(path) = entry["path"].as_str() else { continue };
        let Ok(session) = runtime::Session::load_from_path(path) else {
            skipped += 1;
            continue;
        };
        let Some(history) = replay_support::history_as_it_stood(&session) else {
            skipped += 1;
            continue;
        };
        read += 1;
        for point in replay_support::replay_points(&history, TaskReading::PersonsNewest) {
            let Some(hindsight) = point.hindsight else {
                unlabeled += 1;
                continue;
            };
            let ask = &point.ask;
            rows.push(Row {
                id: format!("{}:{transcript}:{}", PATCH_REVIEW.id, ask.tool_use_id),
                at: session.created_at_ms,
                seq: point.at,
                group: transcript.to_string(),
                label: !hindsight.stood(),
                label_word: hindsight.word(),
                baseline: json!({
                    "tool": ask.tool_name,
                    "hunks": ask.hunks.len(),
                    "patchBytes": unified_diff(&ask.hunks).len(),
                    "linesAdded": count_lines(&ask.hunks, '+'),
                    "linesRemoved": count_lines(&ask.hunks, '-'),
                    "taskChars": ask.task.chars().count(),
                    "evidenceBytes": ask.evidence.len(),
                }),
                state: runtime::patch_review::state(ask),
                shipped: serde_json::to_value(runtime::patch_review::questions()).unwrap_or(Value::Null),
            });
        }
    }
    (rows, read, skipped, unlabeled)
}

/// Every ring of the notify seed the seat's own mark can grade: one the
/// person turned to, or one they were present for and did not. A ring
/// nobody was there for compares nothing (`notify_call::agreed`) and is no
/// row.
fn notify_rows(seed: &Value) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, row) in seed["rows"].as_array().into_iter().flatten().enumerate() {
        let Some(ring) = row["verb"].as_str().and_then(notify::from_verb) else { continue };
        let Some(attendance) = row["attendance"].as_str().and_then(Attendance::from_word) else { continue };
        let Some(reacted) = row["label"]["reacted"].as_bool() else { continue };
        if notify_call::agreed(Call::today(), reacted, attendance).is_err() {
            continue;
        }
        let recent: Vec<Recent> = row["recent"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|one| {
                Some(Recent {
                    ring: one["verb"].as_str().and_then(notify::from_verb)?,
                    interrupted: one["interrupted"].as_bool().unwrap_or(false),
                    ago_ms: one["agoMs"].as_i64().unwrap_or(0),
                    reacted: one["reacted"].as_bool(),
                })
            })
            .collect();
        let look = NotifyLook {
            ring,
            interrupted: row["interrupted"].as_bool().unwrap_or(false),
            agent: row["agent"].as_str().unwrap_or_default(),
            pane: row["pane"].as_str().unwrap_or_default(),
            attendance,
            words: row["words"].as_str().unwrap_or_default(),
            since_last_ms: row["sinceLastMs"].as_i64(),
            waiting_panes: usize::try_from(row["waitingPanes"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX),
            recent: &recent,
        };
        let asked = notify_call::ask(&look);
        rows.push(Row {
            id: format!("{}:{index}", NOTIFY.id),
            at: row["at"].as_u64().unwrap_or(0),
            seq: index,
            group: fingerprint_of(row["pane"].as_str().unwrap_or_default()),
            label: reacted,
            label_word: if reacted { "reacted" } else { "quiet" },
            baseline: json!({
                "todaysRule": Call::today().word(),
                "todaysRings": Call::today().rings(),
                "event": row["verb"],
                "attendance": attendance.word(),
                "waitingPanes": row["waitingPanes"],
                "sinceLastSeconds": row["sinceLastMs"].as_i64().map(|ms| ms.max(0) / 1_000),
                "recentRings": recent.len(),
                "recentReacted": recent.iter().filter(|one| one.reacted == Some(true)).count(),
            }),
            state: asked.state,
            shipped: asked.questions,
        });
    }
    rows
}

/// The seed's rows in time order, thinned to `wanted` spread evenly over that
/// order — never its head, which is one busy day — and then to `only`.
fn sample(mut rows: Vec<Row>, wanted: Option<usize>, only: Option<&[String]>) -> Vec<Row> {
    rows.sort_by_key(|row| (row.at, row.seq));
    if let Some(wanted) = wanted.filter(|wanted| *wanted > 0 && *wanted < rows.len()) {
        let every = rows.len().div_ceil(wanted);
        rows = rows.into_iter().step_by(every).collect();
    }
    if let Some(only) = only {
        rows.retain(|row| only.contains(&row.id));
    }
    rows
}

/// The questions object one row is asked: the seat's own for `"shipped"`,
/// else the round's words built as the wire's types build them. `None` for
/// a round whose words do not make a question.
fn questions_of(spec: &Value, shipped: &Value) -> Option<Value> {
    if spec.as_str() == Some(SHIPPED) {
        return Some(shipped.clone());
    }
    let mut built = BTreeMap::new();
    for (id, one) in spec.as_object()? {
        let instructions = one["instructions"].as_str()?;
        let question = match one["type"].as_str()? {
            "noul" => SystemOneQuestion::noul(instructions, one["yes"].as_str()?, one["no"].as_str()?),
            "score" => SystemOneQuestion::score(instructions, one["levels"].as_array()?.iter().filter_map(Value::as_str)),
            "choice" => SystemOneQuestion::choice(
                instructions,
                one["options"].as_object()?.iter().map(|(name, meaning)| (name.as_str(), meaning.as_str())),
            ),
            _ => return None,
        };
        built.insert(id.clone(), question);
    }
    serde_json::to_value(built).ok()
}

/// One answer as a number a model can read: a Noul's probability of yes; a
/// choice's option probabilities; a score's expected level and its
/// probabilities. `None` for an answer of no readable shape.
fn feature_of(answer: &Value) -> Option<Value> {
    if let Some(yes) = answer.get("noul").and_then(Value::as_f64) {
        return yes.is_finite().then(|| json!(yes));
    }
    match answer.get("type").and_then(Value::as_str)? {
        "choice" => Some(json!({
            "choice": answer["choice"],
            "probabilities": answer["probabilities"],
            "confidence": answer["confidence"],
        })),
        "score" => Some(json!({
            "score": answer["score"],
            "probabilities": answer["probabilities"],
            "confidence": answer["confidence"],
        })),
        _ => None,
    }
}

/// Hash the state after the very same door clearance a paid request uses,
/// without taking a day's place or sending a request. A raw state may differ
/// only in words the wire withholds; those rows are duplicates to the model.
fn duplicate_of(row: &Row, seat: Seat) -> String {
    let settings = JevSettings {
        enabled: true,
        workspaces: vec![WORKSPACE.to_string()],
        daily_requests: None,
        model: zerocode_core::jev::DEFAULT_MODEL.to_string(),
    };
    let asking = Asking { key: true, settings: &settings, workspace: Some(WORKSPACE), sent_today: 0 };
    let body = json!({"state": row.state, "model": SYSTEMONE_MODEL, "questions": row.shipped});
    door::may_send(seat.row(), &asking, body)
        .ok()
        .and_then(|cleared| serde_json::from_slice::<Value>(cleared.bytes()).ok())
        .map_or_else(String::new, |sent| fingerprint_of(&sent["state"].to_string()))
}

/// What one row's request came to.
#[derive(Default)]
struct Asked {
    outcome: String,
    answers: BTreeMap<String, Value>,
    input_tokens: u64,
    requests: u32,
    retries: u32,
    cost_unknown: bool,
    budget_exceeded: bool,
    elapsed_ms: u64,
    model: Option<String>,
    /// The state as the door cleared it — what the wire saw.
    state_sent: Option<Value>,
    /// For the seat's own questions: what the seat itself would have
    /// decided, read by the seat's own reader and turned to the label's
    /// polarity — a patch not permitted predicts regret, a call that rings
    /// predicts a person who turns to it. The "today's question" baseline,
    /// on the same rows.
    shipped_predicts: Option<bool>,
}

/// The seat's own decision on its own answers, as the label reads it.
fn seat_decides(seat: Seat, row: &Row, response: &api::SystemOneResponse) -> Option<bool> {
    match seat {
        Seat::PatchReview => runtime::patch_review::read(response).ok().map(|answers| runtime::patch_review::verdict(Some(&answers)) != Verdict::Permit),
        Seat::Notify => {
            let answers = serde_json::to_value(&response.answers).ok()?;
            NotifyAsk { state: row.state.clone(), questions: row.shipped.clone() }.read(&answers).ok().map(|choice| choice.call.rings())
        }
    }
}

/// Ask one row every question of the round in one request, through the
/// seat's row of the use table.
async fn ask(door: &JevDoor, client: &SystemOneClient, seat: Seat, row: &Row, questions: &Value, shipped: bool, keep_state: bool) -> Asked {
    let body = json!({"state": row.state, "model": SYSTEMONE_MODEL, "questions": questions});
    let Ok(cleared) = door.pass(seat.row(), true, body) else {
        return Asked { outcome: "refused".to_string(), ..Asked::default() };
    };
    let state_sent = keep_state
        .then(|| serde_json::from_slice::<Value>(cleared.bytes()).ok().map(|sent| sent["state"].clone()))
        .flatten();
    let call = client.decide_body_once(cleared.into_bytes(), WALL).await;
    let mut asked = Asked {
        elapsed_ms: jev_gate::millis(call.elapsed), state_sent,
        requests: call.requests, retries: call.retries,
        cost_unknown: call.requests > 0 && call.outcome.is_err(),
        ..Asked::default()
    };
    match call.outcome {
        Err(failure) => asked.outcome = failure.ledger_token(),
        Ok(response) => {
            asked.input_tokens = response.usage.input_tokens;
            if shipped {
                asked.shipped_predicts = seat_decides(seat, row, &response);
            }
            asked.model = Some(response.model);
            let wanted: Vec<&String> = questions.as_object().into_iter().flat_map(|map| map.keys()).collect();
            for id in &wanted {
                if let Some(feature) = response.answers.get(*id).and_then(feature_of) {
                    asked.answers.insert((*id).clone(), feature);
                }
            }
            asked.outcome =
                if asked.answers.len() == wanted.len() { ANSWERED.to_string() } else { api::SystemOneFailure::Schema.token().to_string() };
        }
    }
    asked
}

/// What one run came to, as the summary line and the console say it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Summary {
    pub(super) seat: String,
    pub(super) labeled: usize,
    pub(super) unlabeled: usize,
    pub(super) duplicate_ids: usize,
    pub(super) rows: usize,
    pub(super) asked: usize,
    pub(super) answered: usize,
    pub(super) capped: usize,
    pub(super) questions: usize,
    pub(super) input_tokens: u64,
    pub(super) cost_usd: f64,
    pub(super) cost_unknown: usize,
    pub(super) budget_exceeded: usize,
    pub(super) wall_ms_p50: u64,
    pub(super) wall_ms_p95: u64,
    pub(super) cap: usize,
    pub(super) transcripts_read: usize,
    pub(super) transcripts_skipped: usize,
}

/// The whole of one round: the seed's labeled rows, the sample, one request
/// per row within `cap` and the spend line, and the rows file — numbers,
/// labels and outcomes, never a word of the state.
#[allow(clippy::too_many_lines)]
pub(super) fn run(round: &Round, door: &JevDoor, client: Option<&SystemOneClient>, cap: usize, out: &Path, states: Option<&Path>) -> Summary {
    run_with_spend(round, door, client, cap, spend_line(std::env::var_os(SPEND_ENV).as_deref()), out, states)
}

/// The dollars one asking stage may spend: the line the loop handed over in
/// [`SPEND_ENV`], never above [`SPEND_CAP_USD`]. Only a line nobody handed
/// over — a person running the stage by hand — is the brief's; one handed
/// over that does not read as a non-negative number is no money at all,
/// never a fresh ceiling.
fn spend_line(handed: Option<&std::ffi::OsStr>) -> f64 {
    let Some(handed) = handed else { return SPEND_CAP_USD };
    handed
        .to_str()
        .and_then(|raw| raw.parse::<f64>().ok())
        .filter(|line| line.is_finite() && *line >= 0.0)
        .map_or(0.0, |line| line.min(SPEND_CAP_USD))
}

#[allow(clippy::too_many_lines)]
fn run_with_spend(round: &Round, door: &JevDoor, client: Option<&SystemOneClient>, cap: usize, spend_cap: f64, out: &Path, states: Option<&Path>) -> Summary {
    let seat = Seat::named(&round.seat).unwrap_or_else(|| panic!("{} is not a seat the search asks about", round.seat));
    let seed: Value =
        serde_json::from_str(&std::fs::read_to_string(&round.source).expect("the source seed reads")).expect("the source seed parses");
    let (labeled, read, skipped, unlabeled) = match seat {
        Seat::PatchReview => patch_rows(&seed),
        Seat::Notify => {
            let rows = notify_rows(&seed);
            let unlabeled = seed["rows"].as_array().map_or(0, Vec::len).saturating_sub(rows.len());
            (rows, 0, 0, unlabeled)
        }
    };
    let labeled_count = labeled.len();
    let duplicate_ids = labeled_count.saturating_sub(labeled.iter().map(|row| row.id.as_str()).collect::<HashSet<_>>().len());
    let rows = sample(labeled, round.sample, round.rows.as_deref());
    let rate = api::systemone_rate(SYSTEMONE_MODEL).expect("the table prices the alias");
    let asks_nothing = round.questions.as_object().is_some_and(serde_json::Map::is_empty);
    let uses_current_questions = round.questions.as_str() == Some(SHIPPED);
    let question_count = if uses_current_questions {
        rows.first().and_then(|row| row.shipped.as_object()).map_or(0, serde_json::Map::len)
    } else {
        round.questions.as_object().map_or(0, serde_json::Map::len)
    };

    let mut asked: Vec<Option<Asked>> = (0..rows.len()).map(|_| None).collect();
    let mut spent = 0.0;
    let mut cost_unknown = 0usize;
    let mut budget_exceeded = 0usize;
    let mut requests_sent = 0usize;
    if !asks_nothing {
        assert_ne!(seat, Seat::PatchReview, "patch decision time is unavailable; time-ordered paid evaluation is disabled");
        // A round that asks needs the wire; one that only enumerates never
        // touches it, so a dry run stands without a key.
        let client = client.expect("TYPESAFE_API_KEY in this command's environment");
        for (at, row) in rows.iter().enumerate() {
            if requests_sent >= cap || cost_unknown > 0 || budget_exceeded > 0 {
                break;
            }
            let questions = questions_of(&round.questions, &row.shipped).expect("the round's words make questions");
            let body = json!({"state": row.state, "model": SYSTEMONE_MODEL, "questions": questions});
            let reserve_tokens = u64::try_from(body.to_string().len()).unwrap_or(u64::MAX).saturating_add(TOKEN_RESERVE_OVERHEAD);
            if spent + rate.input_cost_usd(reserve_tokens) > spend_cap {
                break;
            }
            let mut one = api::sync_bridge::run_blocking(ask(door, client, seat, row, &questions, uses_current_questions, states.is_some()));
            requests_sent += usize::try_from(one.requests).unwrap_or(usize::MAX);
            spent += rate.input_cost_usd(one.input_tokens);
            one.budget_exceeded = one.input_tokens > reserve_tokens || spent > spend_cap;
            cost_unknown += usize::from(one.cost_unknown);
            budget_exceeded += usize::from(one.budget_exceeded);
            asked[at] = Some(one);
        }
    }

    let mut lines = Vec::with_capacity(rows.len() + 1);
    let mut state_lines = Vec::new();
    let (mut answered, mut capped, mut input_tokens) = (0usize, 0usize, 0u64);
    let mut walls: Vec<u64> = Vec::new();
    for (row, one) in rows.iter().zip(&asked) {
        let (outcome, answers, tokens, elapsed, model, predicts, requests, retries, unknown, overrun) = match one {
            Some(one) => {
                answered += usize::from(one.outcome == ANSWERED);
                input_tokens += one.input_tokens;
                walls.push(one.elapsed_ms);
                if let (Some(state), true) = (&one.state_sent, states.is_some()) {
                    state_lines.push(json!({"row": row.id, "state": state}).to_string());
                }
                (one.outcome.as_str(), json!(one.answers), Some(one.input_tokens), Some(one.elapsed_ms), one.model.clone(), one.shipped_predicts,
                 Some(one.requests), Some(one.retries), one.cost_unknown, one.budget_exceeded)
            }
            None if asks_nothing => ("unasked", json!({}), None, None, None, None, None, None, false, false),
            None => {
                capped += 1;
                (CAPPED, json!({}), None, None, None, None, None, None, false, false)
            }
        };
        lines.push(
            json!({
                "row": row.id,
                "at": row.at,
                "seq": row.seq,
                "group": row.group,
                "duplicate": duplicate_of(row, seat),
                "label": row.label,
                "shippedPredicts": predicts,
                "labelWord": row.label_word,
                "baseline": row.baseline,
                "outcome": outcome,
                "answers": answers,
                "inputTokens": tokens,
                "elapsedMs": elapsed,
                "costUsd": tokens.filter(|_| !unknown).map(|tokens| rate.input_cost_usd(tokens)),
                "costUnknown": unknown,
                "budgetExceeded": overrun,
                "requests": requests,
                "retries": retries,
                "model": model,
            })
            .to_string(),
        );
    }
    walls.sort_unstable();
    let summary = Summary {
        seat: round.seat.clone(),
        labeled: labeled_count,
        unlabeled,
        duplicate_ids,
        rows: rows.len(),
        asked: requests_sent,
        answered,
        capped,
        questions: question_count,
        input_tokens,
        cost_usd: spent,
        cost_unknown,
        budget_exceeded,
        wall_ms_p50: percentile(&walls, 0.5).unwrap_or(0),
        wall_ms_p95: percentile(&walls, 0.95).unwrap_or(0),
        cap,
        transcripts_read: read,
        transcripts_skipped: skipped,
    };
    lines.push(json!({"summary": summary}).to_string());
    std::fs::write(out, lines.join("\n") + "\n").expect("the rows are written");
    if let Some(states) = states {
        std::fs::write(states, state_lines.join("\n") + "\n").expect("the states are written");
    }
    summary
}

/// The search's own door: one consented workspace, a home of its own, so
/// the person's ledgers and day count never move.
pub(super) fn door(home: &Path) -> JevDoor {
    let settings = JevSettings {
        enabled: true,
        workspaces: vec![WORKSPACE.to_string()],
        daily_requests: None,
        model: zerocode_core::jev::DEFAULT_MODEL.to_string(),
    };
    JevDoor::at(settings, Path::new(WORKSPACE), home)
}

#[test]
#[ignore = "reads this machine's private transcripts and seeds and asks Jev"]
fn the_questions_of_a_round_asked_of_this_machines_rows() {
    let round_path = std::env::var(ROUND_ENV).expect("the round file's path");
    let round: Round = serde_json::from_str(&std::fs::read_to_string(&round_path).expect("the round reads")).expect("the round parses");
    let out = std::env::var(OUT_ENV).expect("where the rows go");
    let states = std::env::var(STATES_ENV).ok();
    let cap: usize = std::env::var(CAP_ENV).ok().and_then(|raw| raw.trim().parse().ok()).unwrap_or(REQUEST_CAP);
    let home = tempfile::tempdir().expect("a door home of its own");
    let door = door(home.path());
    let client = api::SystemOneConfig::from_env().ok().map(api::SystemOneConfig::into_client);
    let summary = run(&round, &door, client.as_ref(), cap, Path::new(&out), states.as_deref().map(Path::new));
    println!(
        "--- question discovery: seat {} round {round_path}\n{}",
        summary.seat,
        serde_json::to_string_pretty(&summary).unwrap_or_default()
    );
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use runtime::{ContentBlock, ConversationMessage, MessageRole};
    use zerocode_core::jev::PATCH_REVIEW_TASK_CHAR_CAP;

    use super::super::jev_mock::Mock;
    use super::*;

    /// A notify seed of three rings: two the person was present for, one
    /// they were away from and never turned to — which the seat's mark
    /// cannot grade.
    fn notify_seed(dir: &Path, words: &str) -> PathBuf {
        let row = |at: u64, attendance: &str, reacted: bool| {
            json!({
                "at": at, "pane": "pane-a", "agent": "claude", "verb": notify::VERB_FINISHED, "interrupted": false,
                "attendance": attendance, "words": words, "sinceLastMs": 4_000, "waitingPanes": 1,
                "recent": [{"verb": notify::VERB_ATTENTION, "interrupted": false, "agoMs": 9_000, "reacted": true}],
                "label": {"reacted": reacted, "afterMs": reacted.then_some(3_000)},
            })
        };
        let seed = json!({"rows": [row(1_000, "present", true), row(2_000, "away", false), row(3_000, "present", false)]});
        let path = dir.join("notify-seed.json");
        std::fs::write(&path, seed.to_string()).expect("the seed is written");
        path
    }

    fn user_text(text: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: text.to_string() }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        }
    }

    fn call(id: &str, tool: &str, input: &str) -> ConversationMessage {
        ConversationMessage::assistant(vec![ContentBlock::ToolUse { id: id.to_string(), name: tool.to_string(), input: input.to_string() }])
    }

    fn result(id: &str, tool: &str, output: &str) -> ConversationMessage {
        ConversationMessage::tool_result(id, tool, output, false)
    }

    /// One transcript on disk: a task longer than the seat's cap, an edit
    /// that wrote one hunk, and a check that ran green after it — a patch
    /// that stood, with a receipt.
    fn patch_seed(dir: &Path, task: &str) -> PathBuf {
        let edit = json!({
            "filePath": "/work/zo/src/flag.rs",
            "structuredPatch": [{"oldStart": 12, "oldLines": 1, "newStart": 12, "newLines": 1, "lines": ["-let flag = old;", "+let flag = new;"]}],
        })
        .to_string();
        let mut session = runtime::Session::new();
        for message in [
            user_text(task),
            call("e1", "edit_file", "{}"),
            result("e1", "edit_file", &edit),
            call("c1", "bash", r#"{"command":"cargo test -p flag"}"#),
            result("c1", "bash", r#"{"stdout":"test result: ok","stderr":""}"#),
        ] {
            session.push_message(message).expect("a message");
        }
        let transcript = dir.join("session-1-0.jsonl");
        session.save_to_path(&transcript).expect("the transcript is written");
        let seed = json!({"regretTurns": zerocode_core::jev::PATCH_REVIEW_REGRET_TURNS, "transcripts": [{"path": transcript}]});
        let path = dir.join("patch-seed.json");
        std::fs::write(&path, seed.to_string()).expect("the seed is written");
        path
    }

    /// A wire that answers every question a request carries with a Noul of
    /// one half, or the choice's first option, or a score of one.
    fn answering_every_question() -> Mock {
        answering_every_question_with_tokens(300)
    }

    fn answering_every_question_with_tokens(input_tokens: u64) -> Mock {
        Mock::answering(move |body| {
            let request: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let mut answers = serde_json::Map::new();
            for (id, question) in request["questions"].as_object().into_iter().flatten() {
                let answer = match question["type"].as_str() {
                    Some("noul") => json!({"type": "noul", "noul": 0.5}),
                    Some("score") => json!({"type": "score", "score": 1.0, "legend": {}, "probabilities": {"0": 0.2, "1": 0.6, "2": 0.2}, "confidence": 0.6}),
                    _ => {
                        let first = question["criteria"].as_object().and_then(|options| options.keys().next().cloned()).unwrap_or_default();
                        json!({"type": "choice", "choice": first, "probabilities": {first.clone(): 1.0}, "confidence": 1.0})
                    }
                };
                answers.insert(id.clone(), answer);
            }
            (200, json!({"model": "jev-test", "answers": answers, "usage": {"input_tokens": input_tokens, "output_tokens": 3}}).to_string())
        })
    }

    fn three_nouls() -> Value {
        json!({
            "asks_a_decision": {"type": "noul", "instructions": "Does `words` ask the person to decide something?", "yes": "It asks.", "no": "It does not."},
            "reports_failure": {"type": "noul", "instructions": "Does `words` report a failure?", "yes": "It does.", "no": "It does not."},
            "urgency": {"type": "score", "instructions": "How urgent is `words`?", "levels": ["Not at all.", "Somewhat.", "Very."]},
        })
    }

    fn rows_of(out: &Path) -> (Vec<Value>, Value) {
        let text = std::fs::read_to_string(out).expect("the rows read");
        let mut rows: Vec<Value> = text.lines().filter(|line| !line.is_empty()).map(|line| serde_json::from_str(line).expect("a row")).collect();
        let summary = rows.pop().expect("a summary line");
        (rows, summary["summary"].clone())
    }

    #[test]
    fn one_request_carries_every_question_of_the_round_and_a_row_carries_a_number_per_question() {
        let dir = tempfile::tempdir().expect("a dir");
        let source = notify_seed(dir.path(), "the build is red, keep going or stop?");
        let mock = answering_every_question();
        let round = Round { seat: NOTIFY.id.to_string(), source: source.display().to_string(), sample: None, rows: None, questions: three_nouls() };
        let out = dir.path().join("rows.jsonl");
        let summary = run(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), REQUEST_CAP, &out, None);
        let requests = mock.requests();
        assert_eq!(requests.len(), 2, "one request per gradable row, and the away-and-quiet ring is no row");
        for body in &requests {
            let body: Value = serde_json::from_str(body).expect("a request body");
            let asked: Vec<&String> = body["questions"].as_object().expect("questions").keys().collect();
            assert_eq!(asked, ["asks_a_decision", "reports_failure", "urgency"], "every question of the round rides one request");
        }
        let (rows, _) = rows_of(&out);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row["outcome"], ANSWERED);
            assert_eq!(row["answers"]["asks_a_decision"], 0.5);
            assert_eq!(row["answers"]["urgency"]["score"], 1.0);
            assert_eq!(row["shippedPredicts"], Value::Null, "a round of the search's own questions has no seat decision");
            assert_eq!(row["group"], fingerprint_of("pane-a"), "a pane's rings are one bundle, named by a fingerprint");
        }
        assert_eq!((summary.asked, summary.answered, summary.questions, summary.labeled), (2, 2, 3, 2));
    }

    #[test]
    fn the_cap_bounds_what_leaves_and_names_the_rows_it_left_unasked() {
        let dir = tempfile::tempdir().expect("a dir");
        let source = notify_seed(dir.path(), "done");
        let mock = answering_every_question();
        let round = Round { seat: NOTIFY.id.to_string(), source: source.display().to_string(), sample: None, rows: None, questions: three_nouls() };
        let out = dir.path().join("rows.jsonl");
        let summary = run(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), 1, &out, None);
        assert_eq!(mock.requests().len(), 1);
        let (rows, _) = rows_of(&out);
        assert_eq!(rows.iter().filter(|row| row["outcome"] == CAPPED).count(), 1);
        assert_eq!((summary.asked, summary.capped), (1, 1));
    }

    #[test]
    fn retryable_failures_use_one_reserved_wire_attempt_and_leave_cost_unknown() {
        for status in [429, 529] {
            let dir = tempfile::tempdir().expect("a dir");
            let source = notify_seed(dir.path(), "done");
            let mock = Mock::serving(status, "{}".to_string());
            let round = Round { seat: NOTIFY.id.to_string(), source: source.display().to_string(), sample: None, rows: None, questions: three_nouls() };
            let out = dir.path().join("rows.jsonl");
            let summary = run(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), 1, &out, None);
            let (rows, _) = rows_of(&out);
            assert_eq!(mock.requests().len(), 1, "status {status}: no unreserved retry");
            assert_eq!((summary.asked, summary.cost_unknown), (1, 1));
            assert_eq!((rows[0]["requests"].as_u64(), rows[0]["retries"].as_u64()), (Some(1), Some(0)));
            assert_eq!(rows[0]["costUsd"], Value::Null);
            assert_eq!(rows[0]["costUnknown"], true);
            assert_eq!(rows[1]["outcome"], CAPPED);
        }
    }

    #[test]
    fn a_second_batch_row_is_not_sent_when_its_spend_reservation_would_exceed_the_balance() {
        let dir = tempfile::tempdir().expect("a dir");
        let source = notify_seed(dir.path(), "done");
        let mock = answering_every_question_with_tokens(8_000);
        let round = Round { seat: NOTIFY.id.to_string(), source: source.display().to_string(), sample: None, rows: None, questions: three_nouls() };
        let out = dir.path().join("rows.jsonl");
        let rate = api::systemone_rate(SYSTEMONE_MODEL).expect("rate");
        let balance = rate.input_cost_usd(16_000);
        let summary = run_with_spend(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), 2, balance, &out, None);
        let (rows, _) = rows_of(&out);
        assert_eq!(mock.requests().len(), 1);
        assert_eq!((summary.asked, summary.capped), (1, 1));
        assert_eq!(summary.budget_exceeded, 0);
        assert_eq!(rows[1]["outcome"], CAPPED);
    }

    /// The loop hands the stage what is left of the run's dollars. A line
    /// that was handed over but does not read as a non-negative number is
    /// no money at all — never the stage's own fresh ceiling; only a line
    /// nobody handed over (a person running the stage by hand) is the
    /// brief's, and no line reaches above it.
    #[test]
    fn a_handed_spend_line_that_does_not_read_is_no_money_and_only_an_absent_one_is_the_briefs() {
        for (handed, want, why) in [
            (None, SPEND_CAP_USD, "nobody handed a line over"),
            (Some("1.25"), 1.25, "the loop's remaining line"),
            (Some("0.0"), 0.0, "nothing is left"),
            (Some("9"), SPEND_CAP_USD, "never above the stage's own ceiling"),
            (Some("-0.5"), 0.0, "a negative line is no money, not a fresh ceiling"),
            (Some("four dollars"), 0.0, "an unreadable line is no money"),
            (Some(""), 0.0, "an empty line is no money"),
            (Some("NaN"), 0.0, "not a number is no money"),
            (Some("inf"), 0.0, "an endless line is no money"),
        ] {
            let got = spend_line(handed.map(std::ffi::OsStr::new));
            assert!((got - want).abs() < f64::EPSILON, "{handed:?} read as {got}: {why}");
        }
    }

    #[test]
    fn server_usage_above_the_reservation_stops_for_reconciliation() {
        let dir = tempfile::tempdir().expect("a dir");
        let source = notify_seed(dir.path(), "done");
        let mock = answering_every_question_with_tokens(20_000);
        let round = Round { seat: NOTIFY.id.to_string(), source: source.display().to_string(), sample: None, rows: None, questions: three_nouls() };
        let out = dir.path().join("rows.jsonl");
        let rate = api::systemone_rate(SYSTEMONE_MODEL).expect("rate");
        let balance = rate.input_cost_usd(18_000);
        let summary = run_with_spend(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), 2, balance, &out, None);
        let (rows, _) = rows_of(&out);
        assert_eq!(mock.requests().len(), 1);
        assert_eq!((summary.asked, summary.budget_exceeded), (1, 1));
        assert_eq!(rows[0]["budgetExceeded"], true);
        assert_eq!(rows[1]["outcome"], CAPPED);
    }

    #[test]
    fn an_empty_round_asks_nothing_and_writes_every_gradable_row_with_its_label_and_baseline() {
        let dir = tempfile::tempdir().expect("a dir");
        let source = notify_seed(dir.path(), "done");
        let mock = answering_every_question();
        let round = Round { seat: NOTIFY.id.to_string(), source: source.display().to_string(), sample: None, rows: None, questions: json!({}) };
        let out = dir.path().join("rows.jsonl");
        let summary = run(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), REQUEST_CAP, &out, None);
        assert!(mock.requests().is_empty(), "an empty round sends nothing");
        let (rows, _) = rows_of(&out);
        assert_eq!(rows.iter().map(|row| (row["label"].as_bool(), row["labelWord"].as_str())).collect::<Vec<_>>(), [(Some(true), Some("reacted")), (Some(false), Some("quiet"))]);
        assert_eq!(rows[0]["baseline"]["todaysRings"], true, "today's rule rides every row as the cheapest reader");
        assert_eq!(rows[0]["outcome"], "unasked");
        assert_eq!((summary.asked, summary.rows), (0, 2));
    }

    #[test]
    fn the_rows_file_carries_no_words_of_the_state_and_the_states_file_carries_what_the_door_cleared() {
        let dir = tempfile::tempdir().expect("a dir");
        let words = "a sentence nobody should find in a numbers file";
        let source = notify_seed(dir.path(), words);
        let mock = answering_every_question();
        let round = Round { seat: NOTIFY.id.to_string(), source: source.display().to_string(), sample: None, rows: None, questions: three_nouls() };
        let out = dir.path().join("rows.jsonl");
        let states = dir.path().join("states.jsonl");
        run(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), REQUEST_CAP, &out, Some(&states));
        let written = std::fs::read_to_string(&out).expect("the rows read");
        assert!(!written.contains(words) && !written.contains("pane-a"), "the rows file is numbers, labels and outcomes:\n{written}");
        let kept = std::fs::read_to_string(&states).expect("the states read");
        assert!(kept.contains(words), "the states file is for the proposer's eyes and carries the state the wire saw");
        assert_eq!(kept.lines().count(), 2);
        let state: Value = serde_json::from_str(kept.lines().next().expect("a state line")).expect("state JSON");
        let (rows, _) = rows_of(&out);
        assert_eq!(rows[0]["duplicate"], fingerprint_of(&state["state"].to_string()), "duplicate identity is the door-cleared model state");
    }

    #[test]
    fn the_sample_is_spread_over_time_and_a_named_row_list_narrows_it() {
        let dir = tempfile::tempdir().expect("a dir");
        let source = notify_seed(dir.path(), "done");
        let mock = answering_every_question();
        let named = format!("{}:2", NOTIFY.id);
        let round = Round { seat: NOTIFY.id.to_string(), source: source.display().to_string(), sample: Some(1), rows: None, questions: json!({}) };
        let out = dir.path().join("rows.jsonl");
        run(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), REQUEST_CAP, &out, None);
        let (rows, _) = rows_of(&out);
        assert_eq!(rows.len(), 1, "a sample of one takes one row, spread from the oldest");
        assert_eq!(rows[0]["row"], format!("{}:0", NOTIFY.id));
        let round = Round { rows: Some(vec![named.clone()]), sample: None, ..round };
        run(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), REQUEST_CAP, &out, None);
        let (rows, _) = rows_of(&out);
        assert_eq!(rows.iter().map(|row| row["row"].as_str().unwrap_or_default().to_string()).collect::<Vec<_>>(), [named]);
    }

    #[test]
    fn patch_rows_are_enumerable_but_paid_time_ordered_evaluation_is_refused() {
        let dir = tempfile::tempdir().expect("a dir");
        let task = "rename the flag to new ".repeat(400);
        assert!(task.chars().count() > PATCH_REVIEW_TASK_CHAR_CAP);
        let source = patch_seed(dir.path(), &task);
        let mock = answering_every_question();
        let round = Round { seat: PATCH_REVIEW.id.to_string(), source: source.display().to_string(), sample: None, rows: None, questions: json!({}) };
        let out = dir.path().join("rows.jsonl");
        let summary = run(&round, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), REQUEST_CAP, &out, None);
        assert!(mock.requests().is_empty());
        let (rows, _) = rows_of(&out);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0]["label"].as_bool(), rows[0]["labelWord"].as_str()), (Some(false), Some("receipt")), "a green check after the edit is a patch that stood");
        assert_eq!(rows[0]["baseline"]["linesAdded"], 1);
        assert_eq!(rows[0]["outcome"], "unasked");
        assert_eq!(rows[0]["group"], "0", "a transcript's patches are one bundle");
        assert_eq!((summary.transcripts_read, summary.questions), (1, 0));
        let paid = Round { questions: SHIPPED.into(), ..round };
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&paid, &door(dir.path()), Some(&SystemOneClient::new(&mock.base_url, "k")), REQUEST_CAP, &out, None)
        }));
        assert!(stopped.is_err() && mock.requests().is_empty());
    }

    #[test]
    fn a_round_of_words_that_make_no_question_is_refused_before_anything_leaves() {
        assert_eq!(questions_of(&json!({"q": {"type": "noul", "instructions": "x"}}), &json!({})), None, "a Noul without its yes and no");
        assert_eq!(questions_of(&json!({"q": {"type": "essay", "instructions": "x"}}), &json!({})), None, "a type the wire has no question for");
        let built = questions_of(&three_nouls(), &json!({})).expect("three questions");
        assert_eq!(built["urgency"]["type"], "score");
        assert_eq!(built["asks_a_decision"]["criteria"]["true"], "It asks.");
    }
}
