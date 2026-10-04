//! A coordinator's mail, triaged by Jev as it arrives (t-9471,
//! `zerocode_core::jev::MAIL_TRIAGE`, `smart.jevMailTriage`).
//!
//! On the beat, for every run whose coordinator seat this window holds, the
//! letters to the coordinator that it can still be handed — waiting in its
//! inbox, or in the batch it holds open — and nobody has asked about are put
//! to Jev TOGETHER (t-32796): one request holds the coordinator's situation
//! once and each letter's structure, never its words, and asks each letter its
//! own question (`zerocode_core::mail_triage::MailTriage`), through the Jev
//! door with the coordinator's own checkout as the workspace consented to. A
//! burst above
//! the cap is cut evenly into requests that leave side by side, a few at a
//! time ([`crate::systemone::TOGETHER_LANES`]), and a wave's rows are written
//! before the next wave leaves. Each letter has a row of its own in
//! `mail-triage.jsonl`; the request's account — its count, its bytes, its
//! tokens — is written on the one row that carries it (the first of its
//! letters that names a task, so the cost of a task reads it) and the rows
//! riding it are charged nothing, so a ledger's sums still say what was sent.
//! The rows change nothing the beat does: the seat only records, and nothing
//! on the desk moves.
//!
//! Later beats read what the coordinator did next off the ledger
//! (`zerocode_core::mail_triage::Mailroom::label`) and write it as the row's
//! label — when the window saw the letter handed over, the batch's own stamp
//! is the label's start. A question waits up to [`MAIL_TRIAGE_DEADLINE`] for
//! its answer, so what a beat found is gathered on the beat and asked in one
//! job off it ([`Host::off_the_beat`]); the beat never waits on a socket, and
//! a ledger that has not moved since the last beat costs a comparison.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::jev::batch::{Request, requests};
use zerocode_core::jev::summary::{
    AGREED, AT, BASELINE_AGREED, ELAPSED_MS, LABEL, NOT_COMPARED, OUTCOME, REQUEST_AT,
    RUBRIC_VERSION,
};
use zerocode_core::jev::{JevMode, MAIL_TRIAGE};
use zerocode_core::mail_triage::{
    Filed, LABEL_HORIZON_MS, Labeled, MAIL_TRIAGE_RUBRIC_VERSION, MailLook, MailRead, MailTriage,
    Mailroom, NotCompared, Situation, Start, Triage, kind_rule,
};
use zerocode_core::orchestration::task_cost::TASK_STAMP;
use zerocode_core::orchestration::{Ledger, Message, Run};

use crate::agent_teams::Host;
use crate::systemone::{Asked, SCHEMA, Wire, request_body};

/// How long one question may wait for its answer — the table's wire wall
/// (`MAIL_TRIAGE_DEADLINE_MS`), read from there so the wait and its reason
/// are one number.
pub(crate) const MAIL_TRIAGE_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::MAIL_TRIAGE_DEADLINE_MS);

/// The key a request row names its letter under, and a label row names it
/// back — the table's own name for the seat's requests.
const KEY: &str = MAIL_TRIAGE.request_name[0];

/// The row's outcome for a question Jev answered in shape.
const ANSWERED: &str = "answered";

/// The keys a request row carries that the tail is read back by — one
/// spelling for the writer and the reader.
const RUN: &str = "run";
const TRIAGE: &str = "triage";
const DELIVERED_MS: &str = "deliveredMs";

/// The keys a row names the request that carried it by: the id of the letter
/// whose row carries the request's account — shared by every row it answered —
/// and how many letters the request asked about.
const BATCH: &str = "batch";
const BATCH_SIZE: &str = "batchSize";

/// The key a row keeps the bytes its request carried under: on the one row
/// that carries the request's account, and `0` on the rows riding it.
const REQUEST_BYTES: &str = "requestBytes";

/// What this window remembers about the letters it asked about.
#[derive(Default)]
pub(super) struct MailBook {
    /// The ledger revision the last sweep read: an unchanged ledger has no new
    /// letter, no new hand-over and no new act, and costs a comparison.
    revision: Option<u64>,
    /// Every letter asked about, by id — once, whatever became of it. Read
    /// back off the rows' tail by the first sweep, so a restart asks about
    /// none of them again.
    asked: HashSet<String>,
    /// When this window saw each letter in the batch its coordinator held
    /// open — the batch's own stamp — by letter, until its label is written.
    opened: HashMap<String, i64>,
    /// The ledger the rows go to, and the answered rows without a label yet.
    /// `None` until a sweep has read that ledger's tail.
    waiting: Option<(PathBuf, Vec<Waiting>)>,
}

impl MailBook {
    /// Read `ledger`'s tail once: the letters asked about before this window
    /// and the answers still waiting for their label.
    fn load(&mut self, ledger: &Path) {
        if self
            .waiting
            .as_ref()
            .is_some_and(|(held, _)| held == ledger)
        {
            return;
        }
        let (asked, waiting) = read_tail(ledger);
        self.asked.extend(asked);
        self.waiting = Some((ledger.to_path_buf(), waiting));
    }

    /// The batch `run`'s coordinator holds open, as this window sees it: its
    /// stamp beside every waiting letter in it, the first time it is seen.
    fn note_open(&mut self, run: &Run) {
        let Some((_, waiting)) = self.waiting.as_ref() else {
            return;
        };
        let Some(batch) = run.open_delivery(&run.address()) else {
            return;
        };
        let Some(opened_ms) = batch.opened_ms else {
            return;
        };
        for id in &batch.messages {
            if waiting.iter().any(|one| one.key == *id) {
                self.opened.entry(id.clone()).or_insert(opened_ms);
            }
        }
    }
}

fn kept(book: &Mutex<MailBook>) -> MutexGuard<'_, MailBook> {
    book.lock().unwrap_or_else(|held| held.into_inner())
}

/// An answered question waiting for what the coordinator does next.
#[derive(Debug, Clone, PartialEq)]
struct Waiting {
    key: String,
    run: String,
    asked_ms: i64,
    triage: Triage,
    /// The batch's stamp, when the letter was already in it when asked.
    delivered_ms: Option<i64>,
}

/// One letter of a batch, as its row keeps it: its facts, never its words.
struct Letter {
    key: String,
    kind: &'static str,
    from: String,
    worker: Option<String>,
    dispatch: Option<String>,
    task: Option<String>,
    created_ms: i64,
    delivered_ms: Option<i64>,
    repeats: usize,
}

impl Letter {
    /// The row's facts of `letter`, which `look` read off the ledger.
    fn of(room: &Mailroom<'_>, letter: &Message, look: &MailLook<'_>) -> Self {
        Self {
            key: letter.id.clone(),
            kind: letter.kind.as_str(),
            from: look.from.to_string(),
            worker: look.worker.map(str::to_string),
            dispatch: letter.dispatch.clone(),
            task: letter.task.clone(),
            created_ms: letter.created_ms,
            delivered_ms: match room.start_of(letter, None) {
                Start::Opened(at) => Some(at),
                Start::Checked(_) | Start::Created(_) => None,
            },
            repeats: look.repeats,
        }
    }
}

/// One run's fresh letters on their way to Jev: what every row of them shares
/// and each letter's own facts — asked together, in as many requests as the
/// cap says ([`requests`]).
struct Batch {
    run: String,
    /// The coordinator's own checkout — the workspace the door asks consent
    /// for.
    workspace: Option<PathBuf>,
    mode: JevMode,
    asked_ms: i64,
    /// What holds for every letter at once: written on every row, and said
    /// once in the request.
    situation: Situation,
    letters: Vec<Letter>,
    /// The letters' entries in the request's state, in the letters' order:
    /// read off the ledger on the beat, made into requests off it.
    facts: Vec<Value>,
}

/// Put every letter the coordinators this window seats can still be handed,
/// and nobody has asked about, to Jev — and write the label of every
/// answered one the ledger now says something about.
pub(super) fn sweep(host: &dyn Host, now_ms: i64) {
    let Some(wire) = host.jev_wire() else {
        return;
    };
    let Some(path) = crate::systemone::ledger_of(&wire, &MAIL_TRIAGE) else {
        return;
    };
    let Some(held) = super::runtime() else {
        return;
    };
    let Ok(image) = held.actor.view() else {
        return;
    };
    {
        let mut book = kept(&held.mail);
        if book.revision == Some(image.revision()) {
            return;
        }
        book.revision = Some(image.revision());
        book.load(&path);
    }
    let Ok(ledger) = super::cached_ledger(&held, &image) else {
        return;
    };
    let teams = crate::agent_teams::teams();
    let seats = super::index_team_seats(&teams);
    drop(teams);
    let seated: Vec<(&Run, u32)> = ledger
        .runs()
        .iter()
        .filter_map(|run| Some((run, seat_term(run, &seats)?)))
        .collect();
    let labels = {
        let mut book = kept(&held.mail);
        for (run, _) in &seated {
            book.note_open(run);
        }
        label_waiting(&mut book, &ledger, now_ms)
    };
    crate::systemone::record_rows(&MAIL_TRIAGE, &path, &labels, now_ms);
    ask_about(host, &wire, &held.mail, &path, &seated, now_ms);
}

/// Gather the fresh letters of every run this window seats — one batch a
/// run — and ask them in one job off the beat, and keep each answer's wait
/// for its label.
fn ask_about(
    host: &dyn Host,
    wire: &Wire,
    book: &Arc<Mutex<MailBook>>,
    path: &Path,
    seated: &[(&Run, u32)],
    now_ms: i64,
) {
    let mut mode = None;
    let mut batches = Vec::new();
    // The question reads no act of the coordinator's: its room holds none.
    let no_receipts = Vec::new();
    for (run, term) in seated {
        let room = Mailroom::of_run(run, &no_receipts);
        let fresh = fresh_letters(&room, &kept(book).asked);
        if fresh.is_empty() {
            continue;
        }
        let mode = *mode.get_or_insert_with(|| MAIL_TRIAGE.mode_in(&wire.settings_root()));
        if !mode.asks() {
            return;
        }
        let coordinator_busy = super::pane_turns()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(term)
            .map(|turn| matches!(turn.as_read(), super::PaneTurn::Running { .. }));
        let situation = room.situation(run, coordinator_busy);
        let mut looks = Vec::with_capacity(fresh.len());
        for letter in &fresh {
            looks.push(room.look(run, letter, now_ms));
        }
        batches.push(Batch {
            run: run.id.clone(),
            workspace: host.worktree_of(*term),
            mode,
            asked_ms: now_ms,
            situation,
            letters: fresh
                .iter()
                .zip(&looks)
                .map(|(letter, look)| Letter::of(&room, letter, look))
                .collect(),
            facts: looks.iter().map(MailLook::facts).collect(),
        });
        kept(book)
            .asked
            .extend(fresh.iter().map(|letter| letter.id.clone()));
    }
    if batches.is_empty() {
        return;
    }
    let wire = wire.clone();
    let path = path.to_path_buf();
    let book = Arc::clone(book);
    host.off_the_beat(Box::new(move || {
        settle(&wire, batches, |rows, waiting| {
            crate::systemone::record_rows(&MAIL_TRIAGE, &path, &rows, now_ms);
            keep_waiting(&book, waiting);
        });
    }));
}

/// Keep each answer's wait for its label. A book that has not read the
/// ledger's tail yet reads these rows there; one that has takes them here,
/// once.
fn keep_waiting(book: &Mutex<MailBook>, waiting: Vec<Waiting>) {
    if let Some((_, held)) = kept(book).waiting.as_mut() {
        for one in waiting {
            if !held.iter().any(|row| row.key == one.key) {
                held.push(one);
            }
        }
    }
}

/// What each letter of a request came to: the answer read in shape, or the
/// word it was refused with — the whole request's when nothing came back,
/// the letter's own when only its answer broke a rule of the closed choice.
fn read_request(
    judgment: &MailTriage,
    request: &Request,
    asked: &Asked,
) -> Vec<Result<MailRead, String>> {
    let answers = asked
        .answer
        .as_ref()
        .map_err(String::clone)
        .and_then(|body| {
            serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|mut parsed| parsed.get_mut("answers").map(Value::take))
                .ok_or_else(|| SCHEMA.to_string())
        });
    match answers {
        Ok(answers) => request
            .read(judgment, &answers)
            .into_iter()
            .map(|read| read.map_err(|refusal| refusal.token().to_string()))
            .collect(),
        Err(token) => request.items().map(|_| Err(token.clone())).collect(),
    }
}

/// The row of one letter, before what came of its question is said.
fn row_of(batch: &Batch, letter: &Letter) -> Value {
    json!({
        (AT.canonical): batch.asked_ms,
        KEY: letter.key,
        RUN: batch.run,
        "kind": letter.kind,
        "from": letter.from,
        "worker": letter.worker,
        "dispatch": letter.dispatch,
        TASK_STAMP: letter.task,
        "mode": batch.mode.key(),
        (RUBRIC_VERSION.canonical): MAIL_TRIAGE_RUBRIC_VERSION,
        "createdMs": letter.created_ms,
        DELIVERED_MS: letter.delivered_ms,
        "repeats": letter.repeats,
        "openQuestions": batch.situation.open_questions,
        "coordinatorBusy": batch.situation.coordinator_busy,
    })
}

/// What an answer in shape writes on its letter's row.
fn answered(row: &mut Value, read: &MailRead) {
    row[OUTCOME.canonical] = json!(ANSWERED);
    row[TRIAGE] = json!(read.triage.word());
    row["probabilities"] = json!(read.probabilities);
    row["confidence"] = json!(read.confidence);
    row["urgent"] = json!(read.urgent);
}

/// Ask every batch's requests side by side, a wave at a time
/// ([`Wire::ask_in_waves`]), and hand `written` each wave's rows — what came of
/// each letter: its row, and, for an answer, the wait for its label — before
/// the next wave leaves, so a window closed meanwhile has lost one wave's rows
/// and not the sweep's.
fn settle(wire: &Wire, batches: Vec<Batch>, mut written: impl FnMut(Vec<Value>, Vec<Waiting>)) {
    let built: Vec<(Batch, MailTriage, Vec<Request>)> = batches
        .into_iter()
        .map(|mut batch| {
            let judgment = MailTriage::new(batch.situation);
            let asked = requests(&judgment, std::mem::take(&mut batch.facts));
            (batch, judgment, asked)
        })
        .collect();
    let asks = built
        .iter()
        .flat_map(|(batch, _, asked)| {
            asked.iter().map(move |request| {
                (
                    batch.workspace.as_deref(),
                    request_body(&request.state, &request.questions),
                )
            })
        })
        .collect();
    // The requests in the order they were handed to the wire, which is the
    // order the waves bring their answers back in.
    let mut owners = built.iter().flat_map(|(batch, judgment, asked)| {
        asked.iter().map(move |request| (batch, judgment, request))
    });
    for wave in wire.ask_in_waves(&MAIL_TRIAGE, asks, MAIL_TRIAGE_DEADLINE) {
        let mut rows = Vec::new();
        let mut waiting = Vec::new();
        for answer in &wave {
            let Some((batch, judgment, request)) = owners.next() else {
                break;
            };
            let (settled, awaiting) = rows_of(batch, judgment, request, answer);
            rows.extend(settled);
            waiting.extend(awaiting);
        }
        written(rows, waiting);
    }
}

/// The rows of one answered request: a row for each of its letters, and the
/// wait for its label of each that was answered in shape. The request's
/// account is written on the one row that carries it and the rows beside it
/// ride it, charged nothing ([`crate::systemone::Spent::rider`]).
fn rows_of(
    batch: &Batch,
    judgment: &MailTriage,
    request: &Request,
    answer: &Asked,
) -> (Vec<Value>, Vec<Waiting>) {
    let readings = read_request(judgment, request, answer);
    // What is the request's, worked out once and not once a letter: its wait,
    // the letter that carries its account — the first that names a task, so
    // the cost of a task reads it as one's and not as nobody's; the first of
    // all when none does — which names the batch for every row the request
    // answered, and what the rows riding it are charged.
    let items = request.items();
    let carrier = items
        .clone()
        .find(|item| batch.letters[*item].task.is_some())
        .unwrap_or(items.start);
    let carrier_key = &batch.letters[carrier].key;
    let riding = answer.spent.rider();
    let waited = u64::try_from(answer.waited.as_millis()).unwrap_or(u64::MAX);
    let mut rows = Vec::with_capacity(items.len());
    let mut waiting = Vec::new();
    for (item, reading) in items.clone().zip(readings) {
        let letter = &batch.letters[item];
        let mut row = row_of(batch, letter);
        row[ELAPSED_MS.canonical] = json!(waited);
        if item == carrier {
            answer.spent.stamp(&mut row);
            row[REQUEST_BYTES] = json!(answer.request_bytes);
        } else {
            riding.stamp(&mut row);
            row[REQUEST_BYTES] = json!(0);
        }
        row[BATCH] = json!(carrier_key);
        row[BATCH_SIZE] = json!(items.len());
        match reading {
            Ok(read) => {
                answered(&mut row, &read);
                waiting.push(Waiting {
                    key: letter.key.clone(),
                    run: batch.run.clone(),
                    asked_ms: batch.asked_ms,
                    triage: read.triage,
                    delivered_ms: letter.delivered_ms,
                });
            }
            Err(token) => row[OUTCOME.canonical] = json!(token),
        }
        rows.push(row);
    }
    (rows, waiting)
}

/// The letters a question may be asked about: those the coordinator can
/// still be handed — waiting in its inbox, or in the batch it holds open —
/// that nobody has asked about yet, oldest first.
fn fresh_letters<'a>(room: &Mailroom<'a>, asked: &HashSet<String>) -> Vec<&'a Message> {
    room.letters()
        .filter(|letter| {
            (room.is_pending(letter) || room.is_open(letter)) && !asked.contains(&letter.id)
        })
        .collect()
}

/// Every letter the rows at `ledger`'s tail asked about, and the answered
/// ones no label row names yet.
fn read_tail(ledger: &Path) -> (HashSet<String>, Vec<Waiting>) {
    let Some(lines) = zerocode_core::transcript::tail_lines(ledger) else {
        return (HashSet::new(), Vec::new());
    };
    let rows: Vec<Value> = lines
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let labeled: HashSet<&str> = rows
        .iter()
        .filter_map(|row| LABEL.read(row).and_then(Value::as_str))
        .collect();
    let mut asked = HashSet::new();
    let mut waiting: Vec<Waiting> = Vec::new();
    for row in &rows {
        let Some(key) = row.get(KEY).and_then(Value::as_str) else {
            continue;
        };
        asked.insert(key.to_string());
        if OUTCOME.read(row).and_then(Value::as_str) != Some(ANSWERED)
            || labeled.contains(key)
            || waiting.iter().any(|one| one.key == key)
        {
            continue;
        }
        let (Some(run), Some(asked_ms), Some(triage)) = (
            row.get(RUN).and_then(Value::as_str),
            AT.read(row).and_then(Value::as_i64),
            row.get(TRIAGE)
                .and_then(Value::as_str)
                .and_then(Triage::from_word),
        ) else {
            continue;
        };
        waiting.push(Waiting {
            key: key.to_string(),
            run: run.to_string(),
            asked_ms,
            triage,
            delivered_ms: row.get(DELIVERED_MS).and_then(Value::as_i64),
        });
    }
    (asked, waiting)
}

/// The label rows `ledger` can write now for the answered rows waiting in
/// `book` — each once, and dropped from the book as it is written. A run the
/// ledger no longer holds cannot say what followed, and its rows stay
/// unlabeled rather than guessed at; a letter not handed over yet waits.
fn label_waiting(book: &mut MailBook, ledger: &Ledger, now_ms: i64) -> Vec<Value> {
    let MailBook {
        waiting, opened, ..
    } = book;
    let Some((_, rows)) = waiting.as_mut() else {
        return Vec::new();
    };
    // The receipts that can speak of a waiting letter: those filed since the
    // oldest of them was written.
    let Some(since) = rows
        .iter()
        .filter_map(|one| {
            ledger
                .run(&one.run)?
                .message(&one.key)
                .map(|letter| letter.created_ms)
        })
        .min()
    else {
        rows.clear();
        return Vec::new();
    };
    let receipts = Filed::of_ledger(ledger, since);
    let mut rooms: HashMap<&str, Mailroom<'_>> = HashMap::new();
    let mut labels = Vec::new();
    rows.retain(|one| {
        let Some(run) = ledger.run(&one.run) else {
            return false;
        };
        let room = rooms
            .entry(run.id.as_str())
            .or_insert_with(|| Mailroom::of_run(run, &receipts));
        let Some(letter) = room.message(&one.key) else {
            return false;
        };
        if room.is_pending(letter) && now_ms.saturating_sub(letter.created_ms) <= LABEL_HORIZON_MS {
            return true;
        }
        let start = room.start_of(letter, opened.get(&one.key).copied().or(one.delivered_ms));
        match room.label(letter, start, now_ms) {
            None => true,
            Some(outcome) => {
                labels.push(label_row(one, letter, start, &outcome, now_ms));
                opened.remove(&one.key);
                false
            }
        }
    });
    labels
}

/// The label row of one answered letter: what the coordinator did, when and
/// how the label knows the hand-over, and the marks — the answer's and the
/// kind rule's — or why there are none.
fn label_row(
    one: &Waiting,
    letter: &Message,
    start: Start,
    outcome: &Result<Labeled<'_>, NotCompared>,
    now_ms: i64,
) -> Value {
    let mut row = json!({
        (AT.canonical): now_ms,
        (LABEL.canonical): one.key,
        (REQUEST_AT.canonical): one.asked_ms,
        RUN: one.run,
        "kind": letter.kind.as_str(),
        "start": start.word(),
        "startMs": start.at(),
    });
    match outcome {
        Ok(labeled) => {
            row["truth"] = json!(labeled.truth.word());
            row["urgentTruth"] = json!(labeled.urgent);
            row["afterActions"] = json!(labeled.after_actions);
            row["afterMs"] = json!(labeled.after_ms);
            row["handledBy"] = json!(labeled.handled_by);
            // The marks the summary counts: the answer against what the
            // coordinator did, and the kind rule against the same.
            row[AGREED.canonical] = json!(one.triage == labeled.truth);
            row[BASELINE_AGREED.canonical] = json!(kind_rule(letter.kind) == labeled.truth);
        }
        Err(why) => row[NOT_COMPARED.canonical] = json!(why.word()),
    }
    row
}

/// The term this window holds `run`'s live coordinator seat (`team/pane`) in,
/// when it holds it.
fn seat_term(run: &Run, seats: &super::TeamSeatIndex) -> Option<u32> {
    let seat = run.coordinator_live()?;
    let (team, pane) = seat.seat.split_once('/')?;
    seats.get(team)?.get(pane).copied()
}

#[cfg(test)]
mod batch_tests;

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use serde_json::json;
    use zerocode_core::jev::summary::{AGREED, BASELINE_AGREED, LABEL, NOT_COMPARED, REQUEST_AT};
    use zerocode_core::mail_triage::{ANSWER_NOW_WITHIN_ACTS, kind_rule};
    use zerocode_core::orchestration::{
        Draft, MessageKind, Priority, ServedAnswer, ServedRow, Text, worker_address,
    };

    use super::*;

    const COORDINATOR: &str = "actor-v1:coordinator";

    /// A window that restarts reads back the letters its rows asked about —
    /// every one, so none is asked again — and the answered ones no label
    /// names yet, with what they answered.
    #[test]
    fn a_restart_finds_the_letters_it_asked_about_and_the_answers_waiting() {
        let dir = tempfile::tempdir().expect("a zo home");
        let ledger = dir.path().join(MAIL_TRIAGE.ledger);
        let row = |key: &str, outcome: &str, triage: &str| {
            json!({ "at": 5, KEY: key, "run": "run-1", "outcome": outcome,
                    "triage": triage, "urgent": 0.4, "deliveredMs": 3 })
        };
        crate::systemone::append_rows(
            &ledger,
            &[
                row("m-1", "answered", "answer_now"),
                row("m-2", "not_consented", "answer_now"),
                row("m-3", "answered", "can_wait"),
                json!({ "at": 9, (LABEL.canonical): "m-1", "truth": "answer_now" }),
            ],
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(&ledger)
            .and_then(|mut file| file.write_all(b"{\"torn\": "))
            .expect("a torn last line");

        let (asked, waiting) = read_tail(&ledger);
        let mut asked: Vec<&str> = asked.iter().map(String::as_str).collect();
        asked.sort_unstable();
        assert_eq!(asked, ["m-1", "m-2", "m-3"], "every letter asked about");
        assert_eq!(
            waiting,
            vec![Waiting {
                key: "m-3".into(),
                run: "run-1".into(),
                asked_ms: 5,
                triage: Triage::CanWait,
                delivered_ms: Some(3),
            }]
        );
        let (none, nothing) = read_tail(&dir.path().join("gone.jsonl"));
        assert!(none.is_empty() && nothing.is_empty());
    }

    /// A ledger with one seated coordinator, a worker carrying a task, and
    /// that worker's question to the coordinator, handed over in a batch.
    fn a_run_with_a_question() -> (Ledger, String, String, String) {
        let mut ledger = Ledger::new();
        let run_id = ledger.create_run("mail", 1);
        ledger
            .seat_coordinator(&run_id, "team/%1", Some(COORDINATOR), 2)
            .expect("seated");
        let task = ledger
            .create_task(&run_id, "spec".into(), "title".into(), vec![], None, 3)
            .expect("a task");
        let started = ledger
            .start_worker(&run_id, "claude", ("team", "%2"), Some(&task), 4)
            .expect("a worker");
        let question = ledger
            .post(
                &run_id,
                Draft {
                    from: worker_address(&started.worker),
                    to: format!("run:{run_id}"),
                    kind: MessageKind::Question,
                    body: Text::from("words the label never reads"),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(task.clone()),
                    dispatch: started.dispatch.clone(),
                },
                10,
            )
            .expect("a question");
        (ledger, run_id, question, started.worker)
    }

    /// The coordinator's answer to `question`, and its receipt.
    fn answered(ledger: &Ledger, run_id: &str, question: &str, worker: &str, at: i64) -> Ledger {
        let mut ledger = Ledger::rebuild(ledger.export()).expect("a copy");
        let reply = ledger
            .post(
                run_id,
                Draft {
                    from: format!("run:{run_id}"),
                    to: worker_address(worker),
                    kind: MessageKind::Question,
                    body: Text::from("yes"),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: Some(question.to_string()),
                    task: None,
                    dispatch: None,
                },
                at,
            )
            .expect("an answer");
        let mut projected = ledger.export();
        projected.served.push(ServedRow {
            caller: Some(Text::from(COORDINATOR)),
            request: Text::from("r-reply"),
            answer: ServedAnswer::Inline(json!({ "messageId": reply }).to_string()),
            fingerprint: None,
            verb: Some("reply".to_string()),
            filed_ms: Some(at),
            expired: false,
        });
        Ledger::rebuild(projected).expect("the ledger with its receipt")
    }

    /// An answered question's label is written once the coordinator acts on
    /// it — the answer's own mark and the kind rule's beside it — and not
    /// before; the letter leaves the book as its label is written.
    #[test]
    fn the_label_is_written_off_the_ledger_once_the_coordinator_acts() {
        let (ledger, run_id, question, worker) = a_run_with_a_question();
        let mut book = MailBook {
            waiting: Some((
                PathBuf::from("mail-triage.jsonl"),
                vec![Waiting {
                    key: question.clone(),
                    run: run_id.clone(),
                    asked_ms: 12,
                    triage: Triage::CanWait,
                    delivered_ms: None,
                }],
            )),
            ..MailBook::default()
        };
        // Still waiting in the coordinator's inbox: nothing to say yet.
        assert!(label_waiting(&mut book, &ledger, 20).is_empty());
        let mut projected = ledger.export();
        let inbox = projected
            .inboxes
            .iter_mut()
            .find(|row| row.run == run_id && row.address == format!("run:{run_id}"))
            .expect("the coordinator's inbox");
        inbox.pending.retain(|id| *id != question);
        inbox.open = Some(zerocode_core::orchestration::Delivery {
            id: "d-1".into(),
            messages: vec![question.clone()],
            holder: Some("team/%1".into()),
            opened_ms: Some(15),
        });
        let handed = Ledger::rebuild(projected).expect("handed over");
        book.opened.insert(question.clone(), 15);
        let answered = answered(&handed, &run_id, &question, &worker, 30);
        let labels = label_waiting(&mut book, &answered, 40);
        assert_eq!(labels.len(), 1, "{labels:?}");
        let label = &labels[0];
        assert_eq!(label[LABEL.canonical], question.as_str());
        assert_eq!(label[REQUEST_AT.canonical], 12);
        assert_eq!(label["truth"], "answer_now");
        assert_eq!(label["afterActions"], 1);
        assert_eq!(
            label["afterMs"], 15,
            "from the batch's stamp the window saw"
        );
        assert_eq!(label["start"], "opened");
        assert_eq!(
            label[AGREED.canonical], false,
            "it said the question could wait"
        );
        assert_eq!(
            label[BASELINE_AGREED.canonical],
            kind_rule(MessageKind::Question) == Triage::AnswerNow
        );
        assert!(!label.to_string().contains("words the label"), "{label}");
        assert!(
            book.waiting
                .as_ref()
                .is_some_and(|(_, rows)| rows.is_empty()),
            "a label is written once"
        );
        assert!(book.opened.is_empty(), "and the stamp goes with it");
        assert!(label_waiting(&mut book, &answered, 50).is_empty());
    }

    /// A label that cannot mark says why, in the words the dashboard names.
    #[test]
    fn a_label_that_cannot_mark_says_why() {
        let (ledger, run_id, question, _) = a_run_with_a_question();
        let run = ledger.run(&run_id).expect("the run");
        let letter = run.message(&question).expect("the letter");
        let one = Waiting {
            key: question.clone(),
            run: run_id.clone(),
            asked_ms: 12,
            triage: Triage::AnswerNow,
            delivered_ms: None,
        };
        let row = label_row(
            &one,
            letter,
            Start::Created(10),
            &Err(NotCompared::Superseded),
            99,
        );
        assert_eq!(row[NOT_COMPARED.canonical], "superseded");
        assert!(row.get(AGREED.canonical).is_none());
        assert!(row.get(BASELINE_AGREED.canonical).is_none());
        let handled = label_row(
            &one,
            letter,
            Start::Created(10),
            &Ok(Labeled {
                truth: Triage::AnswerNow,
                urgent: true,
                after_actions: ANSWER_NOW_WITHIN_ACTS,
                after_ms: 5,
                handled_by: Some("reply"),
            }),
            99,
        );
        assert_eq!(handled[AGREED.canonical], true);
        assert_eq!(handled["urgentTruth"], true);
        assert_eq!(handled["handledBy"], "reply");
        assert!(handled.get(NOT_COMPARED.canonical).is_none());
    }

    /// A question is asked about a letter the coordinator can still be
    /// handed — waiting, or in its open batch — and once; one it has been
    /// handed and put away is history, and its own words are not letters.
    #[test]
    fn only_letters_the_coordinator_can_still_be_handed_are_asked_about_once() {
        let (ledger, run_id, question, _) = a_run_with_a_question();
        let run = ledger.run(&run_id).expect("the run");
        let receipts = Vec::new();
        let room = Mailroom::of_run(run, &receipts);
        let fresh: Vec<&str> = fresh_letters(&room, &HashSet::new())
            .iter()
            .map(|one| one.id.as_str())
            .collect();
        assert_eq!(fresh, [question.as_str()]);
        let asked: HashSet<String> = [question.clone()].into_iter().collect();
        assert!(fresh_letters(&room, &asked).is_empty(), "asked once");

        let mut projected = ledger.export();
        let inbox = projected
            .inboxes
            .iter_mut()
            .find(|row| row.run == run_id && row.address == format!("run:{run_id}"))
            .expect("the coordinator's inbox");
        inbox.pending.clear();
        let history = Ledger::rebuild(projected).expect("put away");
        let run = history.run(&run_id).expect("the run");
        let room = Mailroom::of_run(run, &receipts);
        assert!(
            fresh_letters(&room, &HashSet::new()).is_empty(),
            "a letter handed over and put away is history"
        );
    }
}
